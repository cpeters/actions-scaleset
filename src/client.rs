use std::sync::Arc;

use reqwest::StatusCode;
use serde::Deserialize;
use time::{Duration, OffsetDateTime};
use tokio::sync::{Mutex, RwLock};

use crate::auth::{create_jwt_for_github_app, jwt_expires_at, ActionsAuth, GitHubAppAuth};
use crate::config::{join_url_path, GitHubConfig};
use crate::error::{Error, Result};
use crate::http::{expect_status, HttpOptions, Transport};
use crate::session::MessageSessionClient;
use crate::types::{
    apply_default_label_types, ensure_labels, RunnerGroup, RunnerGroupList, RunnerReference,
    RunnerReferenceList, RunnerScaleSet, RunnerScaleSetJitRunnerConfig,
    RunnerScaleSetJitRunnerSetting, RunnerScaleSetsResponse, SystemInfo, RUNNER_ENDPOINT,
    SCALE_SET_ENDPOINT,
};

#[derive(Clone)]
pub struct Client {
    pub(crate) inner: Arc<ClientInner>,
}

pub(crate) struct ClientInner {
    pub transport: RwLock<Transport>,
    pub creds: ActionsAuth,
    pub config: GitHubConfig,
    pub admin: RwLock<AdminToken>,
    pub refresh: Mutex<()>,
}

#[derive(Clone, Default)]
pub(crate) struct AdminToken {
    pub token: String,
    pub authorization_header: String,
    pub expires_at: Option<OffsetDateTime>,
    pub url: String,
}

pub struct GitHubAppClientConfig {
    pub github_config_url: String,
    pub github_app_auth: GitHubAppAuth,
    pub system_info: SystemInfo,
}

pub struct PersonalAccessTokenConfig {
    pub github_config_url: String,
    pub personal_access_token: String,
    pub system_info: SystemInfo,
}

impl Client {
    pub fn with_github_app(config: GitHubAppClientConfig, options: HttpOptions) -> Result<Self> {
        Self::new(
            config.system_info,
            &config.github_config_url,
            ActionsAuth::App(config.github_app_auth),
            options,
        )
    }

    pub fn with_personal_access_token(
        config: PersonalAccessTokenConfig,
        options: HttpOptions,
    ) -> Result<Self> {
        Self::new(
            config.system_info,
            &config.github_config_url,
            ActionsAuth::Token(config.personal_access_token),
            options,
        )
    }

    fn new(
        system_info: SystemInfo,
        github_config_url: &str,
        creds: ActionsAuth,
        options: HttpOptions,
    ) -> Result<Self> {
        let config = GitHubConfig::parse(github_config_url)?;
        creds.validate()?;
        let transport = Transport::new(system_info, options)?;
        Ok(Self {
            inner: Arc::new(ClientInner {
                transport: RwLock::new(transport),
                creds,
                config,
                admin: RwLock::new(AdminToken::default()),
                refresh: Mutex::new(()),
            }),
        })
    }

    pub async fn set_system_info(&self, info: SystemInfo) {
        self.inner.transport.write().await.set_system_info(info);
    }

    pub async fn system_info(&self) -> SystemInfo {
        self.transport_snapshot().await.system_info.clone()
    }

    pub(crate) async fn transport_snapshot(&self) -> Transport {
        self.inner.transport.read().await.clone()
    }

    pub fn github_config(&self) -> &GitHubConfig {
        &self.inner.config
    }

    pub async fn get_runner_scale_set(
        &self,
        runner_group_id: i32,
        name: &str,
    ) -> Result<Option<RunnerScaleSet>> {
        let url = self
            .actions_url(
                SCALE_SET_ENDPOINT,
                &[
                    ("runnerGroupId", runner_group_id.to_string()),
                    ("name", name.to_string()),
                ],
            )
            .await?;
        let resp = self.get(&url).await?;
        let resp = expect_status(resp, StatusCode::OK, "GET", url.as_str()).await?;
        let list: RunnerScaleSetsResponse = resp.json().await?;
        match list.count {
            1 => Ok(list.runner_scale_sets.into_iter().next()),
            0 => Ok(None),
            _ => Err(Error::message(format!(
                "multiple runner scale sets found with name {name:?}"
            ))),
        }
    }

    pub async fn list_runner_scale_sets(
        &self,
        runner_group_id: i32,
    ) -> Result<Vec<RunnerScaleSet>> {
        let url = self
            .actions_url(
                SCALE_SET_ENDPOINT,
                &[("runnerGroupId", runner_group_id.to_string())],
            )
            .await?;
        let resp = self.get(&url).await?;
        let resp = expect_status(resp, StatusCode::OK, "GET", url.as_str()).await?;
        let list: RunnerScaleSetsResponse = resp.json().await?;
        Ok(list.runner_scale_sets)
    }

    pub async fn get_runner_scale_set_by_id(&self, id: i32) -> Result<RunnerScaleSet> {
        let path = format!("/{SCALE_SET_ENDPOINT}/{id}");
        let url = self.actions_url(&path, &[]).await?;
        let resp = self.get(&url).await?;
        let resp = expect_status(resp, StatusCode::OK, "GET", url.as_str()).await?;
        Ok(resp.json().await?)
    }

    pub async fn get_runner_group_by_name(&self, name: &str) -> Result<RunnerGroup> {
        let url = self
            .actions_url(
                "/_apis/runtime/runnergroups/",
                &[("groupName", name.to_string())],
            )
            .await?;
        let resp = self.get(&url).await?;
        let resp = expect_status(resp, StatusCode::OK, "GET", url.as_str()).await?;
        let list: RunnerGroupList = resp.json().await?;
        match list.count {
            1 => list
                .runner_groups
                .into_iter()
                .next()
                .ok_or_else(|| Error::message("empty runner group list")),
            0 => Err(Error::message(format!(
                "no runner group found with name {name:?}"
            ))),
            _ => Err(Error::message(format!(
                "multiple runner group found with name {name:?}"
            ))),
        }
    }

    pub async fn create_runner_scale_set(
        &self,
        mut scale_set: RunnerScaleSet,
    ) -> Result<RunnerScaleSet> {
        ensure_labels(&mut scale_set)?;
        apply_default_label_types(&mut scale_set);
        let url = self.actions_url(SCALE_SET_ENDPOINT, &[]).await?;
        let resp = self.post_json(&url, &scale_set).await?;
        let resp = expect_status(resp, StatusCode::OK, "POST", url.as_str()).await?;
        Ok(resp.json().await?)
    }

    pub async fn update_runner_scale_set(
        &self,
        id: i32,
        mut scale_set: RunnerScaleSet,
    ) -> Result<RunnerScaleSet> {
        apply_default_label_types(&mut scale_set);
        let path = format!("{SCALE_SET_ENDPOINT}/{id}");
        let url = self.actions_url(&path, &[]).await?;
        let resp = self.patch_json(&url, &scale_set).await?;
        let resp = expect_status(resp, StatusCode::OK, "PATCH", url.as_str()).await?;
        Ok(resp.json().await?)
    }

    pub async fn delete_runner_scale_set(&self, id: i32) -> Result<()> {
        let path = format!("/{SCALE_SET_ENDPOINT}/{id}");
        let url = self.actions_url(&path, &[]).await?;
        let resp = self.delete(&url).await?;
        expect_status(resp, StatusCode::NO_CONTENT, "DELETE", url.as_str()).await?;
        Ok(())
    }

    pub async fn generate_jit_runner_config(
        &self,
        setting: &RunnerScaleSetJitRunnerSetting,
        scale_set_id: i32,
    ) -> Result<RunnerScaleSetJitRunnerConfig> {
        let path = format!("/{SCALE_SET_ENDPOINT}/{scale_set_id}/generatejitconfig");
        let url = self.actions_url(&path, &[]).await?;
        let resp = self.post_json(&url, setting).await?;
        let resp = expect_status(resp, StatusCode::OK, "POST", url.as_str()).await?;
        Ok(resp.json().await?)
    }

    pub async fn get_runner(&self, runner_id: i32) -> Result<RunnerReference> {
        let path = format!("/{RUNNER_ENDPOINT}/{runner_id}");
        let url = self.actions_url(&path, &[]).await?;
        let resp = self.get(&url).await?;
        let resp = expect_status(resp, StatusCode::OK, "GET", url.as_str()).await?;
        Ok(resp.json().await?)
    }

    pub async fn get_runner_by_name(&self, name: &str) -> Result<Option<RunnerReference>> {
        let url = self
            .actions_url(RUNNER_ENDPOINT, &[("agentName", name.to_string())])
            .await?;
        let resp = self.get(&url).await?;
        let resp = expect_status(resp, StatusCode::OK, "GET", url.as_str()).await?;
        let list: RunnerReferenceList = resp.json().await?;
        match list.count {
            1 => Ok(list.runner_references.into_iter().next()),
            0 => Ok(None),
            _ => Err(Error::message(format!(
                "multiple runners found with name {name:?}"
            ))),
        }
    }

    pub async fn remove_runner(&self, runner_id: i64) -> Result<()> {
        let path = format!("/{RUNNER_ENDPOINT}/{runner_id}");
        let url = self.actions_url(&path, &[]).await?;
        let resp = self.delete(&url).await?;
        expect_status(resp, StatusCode::NO_CONTENT, "DELETE", url.as_str()).await?;
        Ok(())
    }

    pub async fn message_session_client(
        &self,
        runner_scale_set_id: i32,
        owner: impl Into<String>,
    ) -> Result<MessageSessionClient> {
        MessageSessionClient::create(self.clone(), runner_scale_set_id, owner.into()).await
    }

    pub(crate) async fn actions_url(
        &self,
        path: &str,
        extra_query: &[(&str, String)],
    ) -> Result<String> {
        let token = self.update_token_if_needed().await?;
        let mut assembled = join_url_path(&token.url, path);
        let mut pairs: Vec<(String, String)> = extra_query
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        if !pairs.iter().any(|(k, _)| k == "api-version") {
            pairs.push(("api-version".into(), crate::types::API_VERSION.into()));
        }
        if let Some(idx) = assembled.find('?') {
            let existing = &assembled[idx + 1..];
            if existing.contains("api-version=") {
                pairs.retain(|(k, _)| k != "api-version");
            }
        }
        if !pairs.is_empty() {
            if !assembled.contains('?') {
                assembled.push('?');
            } else if !assembled.ends_with('?') && !assembled.ends_with('&') {
                assembled.push('&');
            }
            let qs: String = pairs
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding_lite(k), urlencoding_lite(v)))
                .collect::<Vec<_>>()
                .join("&");
            assembled.push_str(&qs);
        }
        Ok(assembled)
    }

    pub(crate) async fn admin_authorization(&self) -> Result<String> {
        Ok(self.update_token_if_needed().await?.authorization_header)
    }

    async fn get(&self, url: &str) -> Result<reqwest::Response> {
        let auth = self.admin_authorization().await?;
        let t = self.transport_snapshot().await;
        let builder = t
            .http
            .get(url)
            .header("Authorization", auth)
            .header("User-Agent", &t.user_agent)
            .header("Content-Type", "application/json");
        t.send(builder).await
    }

    async fn delete(&self, url: &str) -> Result<reqwest::Response> {
        let auth = self.admin_authorization().await?;
        let t = self.transport_snapshot().await;
        let builder = t
            .http
            .delete(url)
            .header("Authorization", auth)
            .header("User-Agent", &t.user_agent)
            .header("Content-Type", "application/json");
        t.send(builder).await
    }

    async fn post_json<T: serde::Serialize>(
        &self,
        url: &str,
        body: &T,
    ) -> Result<reqwest::Response> {
        let auth = self.admin_authorization().await?;
        let t = self.transport_snapshot().await;
        let builder = t
            .http
            .post(url)
            .header("Authorization", auth)
            .header("User-Agent", &t.user_agent)
            .json(body);
        t.send(builder).await
    }

    async fn patch_json<T: serde::Serialize>(
        &self,
        url: &str,
        body: &T,
    ) -> Result<reqwest::Response> {
        let auth = self.admin_authorization().await?;
        let t = self.transport_snapshot().await;
        let builder = t
            .http
            .patch(url)
            .header("Authorization", auth)
            .header("User-Agent", &t.user_agent)
            .json(body);
        t.send(builder).await
    }

    pub(crate) async fn post_actions_raw(
        &self,
        url: &str,
        bearer: &str,
        body: Vec<u8>,
    ) -> Result<reqwest::Response> {
        let t = self.transport_snapshot().await;
        let builder = t
            .http
            .post(url)
            .header("Authorization", bearer)
            .header("User-Agent", &t.user_agent)
            .header("Content-Type", "application/json")
            .body(body);
        t.send(builder).await
    }

    async fn update_token_if_needed(&self) -> Result<AdminToken> {
        if let Some(token) = self.snapshot_token().await {
            return Ok(token);
        }
        let _guard = self.inner.refresh.lock().await;
        if let Some(token) = self.snapshot_token().await {
            return Ok(token);
        }
        tracing::info!(
            github_config_url = self.inner.config.config_url.as_str(),
            "refreshing actions service admin token"
        );
        let rt = self.get_runner_registration_token().await?;
        let admin = self.get_actions_service_admin_connection(&rt).await?;
        let expires_at = jwt_expires_at(&admin.token)?;
        let token = AdminToken {
            authorization_header: format!("Bearer {}", admin.token),
            token: admin.token,
            expires_at: Some(expires_at),
            url: admin.url,
        };
        *self.inner.admin.write().await = token.clone();
        Ok(token)
    }

    async fn snapshot_token(&self) -> Option<AdminToken> {
        let token = self.inner.admin.read().await.clone();
        let exp = token.expires_at?;
        if OffsetDateTime::now_utc() + Duration::seconds(60) >= exp {
            return None;
        }
        if token.token.is_empty() {
            return None;
        }
        Some(token)
    }

    async fn get_runner_registration_token(&self) -> Result<String> {
        let path = self.inner.config.registration_token_path()?;
        let url = self.inner.config.github_api_url(&path);
        let bearer = match &self.inner.creds {
            ActionsAuth::Token(t) => format!("Bearer {t}"),
            ActionsAuth::App(app) => {
                let access = self.fetch_installation_token(app).await?;
                format!("Bearer {access}")
            }
        };
        let t = self.transport_snapshot().await;
        let builder = t
            .http
            .post(url.as_str())
            .header("Authorization", bearer)
            .header("User-Agent", &t.user_agent)
            .header("Content-Type", "application/vnd.github.v3+json")
            .body("{}");
        let resp = t.send(builder).await?;
        let resp = expect_status(resp, StatusCode::CREATED, "POST", url.as_str()).await?;
        #[derive(Deserialize)]
        struct RegistrationToken {
            token: Option<String>,
        }
        let parsed: RegistrationToken = resp.json().await?;
        parsed
            .token
            .ok_or_else(|| Error::message("registration token missing token field"))
    }

    async fn fetch_installation_token(&self, app: &GitHubAppAuth) -> Result<String> {
        let jwt = create_jwt_for_github_app(app)?;
        let path = format!("/app/installations/{}/access_tokens", app.installation_id);
        let url = self.inner.config.github_api_url(&path);
        let t = self.transport_snapshot().await;
        let builder = t
            .http
            .post(url.as_str())
            .header("Authorization", format!("Bearer {jwt}"))
            .header("User-Agent", &t.user_agent)
            .header("Content-Type", "application/vnd.github+json");
        let resp = t.send(builder).await?;
        let resp = expect_status(resp, StatusCode::CREATED, "POST", url.as_str()).await?;
        #[derive(Deserialize)]
        struct AccessToken {
            token: String,
        }
        Ok(resp.json::<AccessToken>().await?.token)
    }

    async fn get_actions_service_admin_connection(
        &self,
        registration_token: &str,
    ) -> Result<AdminConnection> {
        let url = self
            .inner
            .config
            .github_api_url("/actions/runner-registration");
        let body = serde_json::json!({
            "url": self.inner.config.config_url.as_str(),
            "runner_event": "register",
        });
        let t = self.transport_snapshot().await;
        let builder = t
            .http
            .post(url.as_str())
            .header("Authorization", format!("RemoteAuth {registration_token}"))
            .header("User-Agent", &t.user_agent)
            .header("Content-Type", "application/json")
            .json(&body);
        let resp = t.send(builder).await?;
        if !resp.status().is_success() {
            return Err(crate::http::read_error_body(resp, "POST", url.as_str(), None).await);
        }
        let parsed: AdminConnection = resp.json().await?;
        if parsed.url.is_empty() || parsed.token.is_empty() {
            return Err(Error::message(
                "actions service admin connection missing url or token",
            ));
        }
        Ok(parsed)
    }
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Clone, Deserialize)]
struct AdminConnection {
    #[serde(default)]
    url: String,
    #[serde(default)]
    token: String,
}
