use async_trait::async_trait;
use reqwest::StatusCode;
use tokio::sync::{Mutex, RwLock};
use url::Url;
use uuid::Uuid;

use crate::client::Client;
use crate::error::{Kind, Result};
use crate::http::{expect_status, read_error_body, Transport};
use crate::types::{
    parse_runner_scale_set_message, AcquireJobsResponse, RunnerScaleSetMessage,
    RunnerScaleSetMessageResponse, RunnerScaleSetSession, API_VERSION,
    HEADER_SCALE_SET_MAX_CAPACITY, SCALE_SET_ENDPOINT,
};

/// Queue operations used by [`crate::listener::Listener`].
#[async_trait]
pub trait SessionApi: Send + Sync {
    async fn get_message(
        &self,
        last_message_id: i32,
        max_capacity: i32,
    ) -> Result<Option<RunnerScaleSetMessage>>;
    async fn delete_message(&self, message_id: i32) -> Result<()>;
    async fn acquire_jobs(&self, request_ids: &[i64]) -> Result<Vec<i64>>;
    async fn session(&self) -> RunnerScaleSetSession;
}

pub struct MessageSessionClient {
    inner: Client,
    transport: Transport,
    scale_set_id: i32,
    owner: String,
    session: RwLock<RunnerScaleSetSession>,
    refresh: Mutex<()>,
}

impl MessageSessionClient {
    pub(crate) async fn create(
        inner: Client,
        scale_set_id: i32,
        owner: String,
        transport: Transport,
    ) -> Result<Self> {
        let client = Self {
            inner,
            transport,
            scale_set_id,
            owner,
            session: RwLock::new(RunnerScaleSetSession::default()),
            refresh: Mutex::new(()),
        };

        client.create_session().await?;

        Ok(client)
    }

    pub async fn close(&self) -> Result<()> {
        let session = self.current_session().await;
        self.delete_session(session.session_id).await
    }

    pub async fn current_session(&self) -> RunnerScaleSetSession {
        self.session.read().await.clone()
    }

    async fn create_session(&self) -> Result<()> {
        let path = format!("/{SCALE_SET_ENDPOINT}/{}/sessions", self.scale_set_id);

        let url = self.inner.actions_url(&path, &[]).await?;

        let body = serde_json::json!({
            "ownerName": self.owner
        });

        let auth = self.inner.admin_authorization().await?;

        let builder = self
            .transport
            .http
            .post(&url)
            .header("Authorization", auth)
            .header("User-Agent", &self.transport.user_agent)
            .header("Content-Type", "application/json")
            .json(&body);

        let response = self.transport.send(builder).await?;

        let response = expect_status(response, StatusCode::OK, "POST", url.as_str()).await?;

        let created: RunnerScaleSetSession = response.json().await?;

        *self.session.write().await = created;

        Ok(())
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        let path = format!(
            "/{SCALE_SET_ENDPOINT}/{}/sessions/{session_id}",
            self.scale_set_id
        );

        let url = self.inner.actions_url(&path, &[]).await?;
        let auth = self.inner.admin_authorization().await?;

        let builder = self
            .transport
            .http
            .delete(&url)
            .header("Authorization", auth)
            .header("User-Agent", &self.transport.user_agent)
            .header("Content-Type", "application/json");

        let response = self.transport.send(builder).await?;

        expect_status(response, StatusCode::NO_CONTENT, "DELETE", url.as_str()).await?;

        Ok(())
    }

    async fn refresh_session(&self, expired: &RunnerScaleSetSession) -> Result<()> {
        let _guard = self.refresh.lock().await;

        let current = self.current_session().await;

        if current.session_id != expired.session_id
            || current.message_queue_access_token != expired.message_queue_access_token
        {
            return Ok(());
        }

        let path = format!(
            "/{SCALE_SET_ENDPOINT}/{}/sessions/{}",
            self.scale_set_id, current.session_id
        );

        let url = self.inner.actions_url(&path, &[]).await?;
        let auth = self.inner.admin_authorization().await?;

        let builder = self
            .transport
            .http
            .patch(&url)
            .header("Authorization", auth)
            .header("User-Agent", &self.transport.user_agent)
            .header("Content-Type", "application/json");

        let response = self.transport.send(builder).await?;

        let response = expect_status(response, StatusCode::OK, "PATCH", url.as_str()).await?;

        *self.session.write().await = response.json().await?;

        Ok(())
    }

    async fn get_message_once(
        &self,
        session: &RunnerScaleSetSession,
        last_message_id: i32,
        max_capacity: i32,
    ) -> Result<Option<RunnerScaleSetMessage>> {
        let mut url = Url::parse(&session.message_queue_url)?;

        if last_message_id > 0 {
            url.query_pairs_mut()
                .append_pair("lastMessageId", &last_message_id.to_string());
        }

        let builder = self
            .transport
            .http
            .get(url.as_str())
            .header(
                "Accept",
                format!("application/json; api-version={API_VERSION}"),
            )
            .header(
                "Authorization",
                format!("Bearer {}", session.message_queue_access_token),
            )
            .header("User-Agent", &self.transport.user_agent)
            .header(HEADER_SCALE_SET_MAX_CAPACITY, max_capacity.to_string());

        let response = self.transport.send(builder).await?;

        match response.status() {
            StatusCode::ACCEPTED => Ok(None),

            StatusCode::OK => {
                let parsed: RunnerScaleSetMessageResponse = response.json().await?;

                Ok(Some(parse_runner_scale_set_message(parsed)?))
            }

            StatusCode::UNAUTHORIZED => Err(read_error_body(
                response,
                "GET",
                url.as_str(),
                Some(Kind::MessageQueueTokenExpired),
            )
            .await),

            _ => Err(read_error_body(response, "GET", url.as_str(), None).await),
        }
    }

    async fn delete_message_once(
        &self,
        session: &RunnerScaleSetSession,
        message_id: i32,
    ) -> Result<()> {
        let mut url = Url::parse(&session.message_queue_url)?;

        let mut path = url.path().trim_end_matches('/').to_string();

        path.push('/');
        path.push_str(&message_id.to_string());

        url.set_path(&path);

        let builder = self
            .transport
            .http
            .delete(url.as_str())
            .header("Content-Type", "application/json")
            .header(
                "Authorization",
                format!("Bearer {}", session.message_queue_access_token),
            )
            .header("User-Agent", &self.transport.user_agent);

        let response = self.transport.send(builder).await?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(()),

            StatusCode::UNAUTHORIZED => Err(read_error_body(
                response,
                "DELETE",
                url.as_str(),
                Some(Kind::MessageQueueTokenExpired),
            )
            .await),

            _ => Err(read_error_body(response, "DELETE", url.as_str(), None).await),
        }
    }

    async fn acquire_jobs_once(
        &self,
        session: &RunnerScaleSetSession,
        request_ids: &[i64],
    ) -> Result<Vec<i64>> {
        let path = format!("/{SCALE_SET_ENDPOINT}/{}/acquirejobs", self.scale_set_id);

        let url = self.inner.actions_url(&path, &[]).await?;
        let body = serde_json::to_vec(request_ids)?;

        let builder = self
            .transport
            .http
            .post(&url)
            .header(
                "Authorization",
                format!("Bearer {}", session.message_queue_access_token),
            )
            .header("User-Agent", &self.transport.user_agent)
            .header("Content-Type", "application/json")
            .body(body);

        let response = self.transport.send(builder).await?;

        match response.status() {
            StatusCode::UNAUTHORIZED => Err(read_error_body(
                response,
                "POST",
                url.as_str(),
                Some(Kind::MessageQueueTokenExpired),
            )
            .await),

            StatusCode::OK => {
                let parsed: AcquireJobsResponse = response.json().await?;

                Ok(parsed.value)
            }

            _ => Err(read_error_body(response, "POST", url.as_str(), None).await),
        }
    }
}

#[async_trait]
impl SessionApi for MessageSessionClient {
    async fn get_message(
        &self,
        last_message_id: i32,
        max_capacity: i32,
    ) -> Result<Option<RunnerScaleSetMessage>> {
        let session = self.current_session().await;

        match self
            .get_message_once(&session, last_message_id, max_capacity)
            .await
        {
            Ok(message) => Ok(message),

            Err(err) if err.is_message_queue_token_expired() => {
                self.refresh_session(&session)
                    .await
                    .map_err(|err| err.context("failed to refresh message session"))?;

                let session = self.current_session().await;

                self.get_message_once(&session, last_message_id, max_capacity)
                    .await
            }

            Err(err) => Err(err.context("failed to get next message")),
        }
    }

    async fn delete_message(&self, message_id: i32) -> Result<()> {
        let session = self.current_session().await;

        match self.delete_message_once(&session, message_id).await {
            Ok(()) => Ok(()),

            Err(err) if err.is_message_queue_token_expired() => {
                self.refresh_session(&session)
                    .await
                    .map_err(|err| err.context("failed to refresh message session"))?;

                let session = self.current_session().await;

                self.delete_message_once(&session, message_id).await
            }

            Err(err) => Err(err.context("failed to delete message")),
        }
    }

    async fn acquire_jobs(&self, request_ids: &[i64]) -> Result<Vec<i64>> {
        let session = self.current_session().await;

        match self.acquire_jobs_once(&session, request_ids).await {
            Ok(ids) => Ok(ids),

            Err(err) if err.is_message_queue_token_expired() => {
                self.refresh_session(&session)
                    .await
                    .map_err(|err| err.context("failed to refresh message session"))?;

                let session = self.current_session().await;

                self.acquire_jobs_once(&session, request_ids).await
            }

            Err(err) => Err(err.context("failed to acquire jobs")),
        }
    }

    async fn session(&self) -> RunnerScaleSetSession {
        self.current_session().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use time::{Duration, OffsetDateTime};
    use wiremock::{
        matchers::{header, method, path},
        Mock, MockServer, Request, ResponseTemplate,
    };

    use crate::client::{AdminToken, PersonalAccessTokenConfig};
    use crate::http::HttpOptions;
    use crate::types::SystemInfo;

    const SCALE_SET_ID: i32 = 42;

    async fn test_session_client(
        server: &MockServer,
        session: RunnerScaleSetSession,
    ) -> MessageSessionClient {
        let client = Client::with_personal_access_token(
            PersonalAccessTokenConfig {
                github_config_url: format!("{}/octo-org", server.uri()),
                personal_access_token: "github-pat".to_string(),
                system_info: SystemInfo::default(),
            },
            HttpOptions {
                retry_max: 0,
                ..HttpOptions::default()
            },
        )
        .unwrap();

        *client.inner.admin.write().await = AdminToken {
            token: "admin-token".to_string(),
            authorization_header: "Bearer admin-token".to_string(),
            expires_at: Some(OffsetDateTime::now_utc() + Duration::hours(1)),
            url: server.uri(),
        };

        let transport = client.transport_snapshot().await;

        MessageSessionClient {
            inner: client,
            transport,
            scale_set_id: SCALE_SET_ID,
            owner: "test-owner".to_string(),
            session: RwLock::new(session),
            refresh: Mutex::new(()),
        }
    }

    fn initial_session(server: &MockServer, session_id: Uuid) -> RunnerScaleSetSession {
        RunnerScaleSetSession {
            session_id,
            owner_name: "test-owner".to_string(),
            message_queue_url: format!("{}/queue", server.uri()),
            message_queue_access_token: "expired-token".to_string(),
            ..RunnerScaleSetSession::default()
        }
    }

    fn refreshed_session(server: &MockServer, session_id: Uuid) -> RunnerScaleSetSession {
        RunnerScaleSetSession {
            session_id,
            owner_name: "test-owner".to_string(),
            message_queue_url: format!("{}/queue", server.uri()),
            message_queue_access_token: "fresh-token".to_string(),
            ..RunnerScaleSetSession::default()
        }
    }

    async fn mock_session_refresh(server: &MockServer, session: &RunnerScaleSetSession) {
        let refresh_path = format!(
            "/{SCALE_SET_ENDPOINT}/{SCALE_SET_ID}/sessions/{}",
            session.session_id
        );

        Mock::given(method("PATCH"))
            .and(path(refresh_path))
            .and(header("authorization", "Bearer admin-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(session))
            .expect(1)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn get_message_refreshes_expired_session_and_retries() {
        let server = MockServer::start().await;
        let session_id = Uuid::new_v4();

        let initial = initial_session(&server, session_id);
        let refreshed = refreshed_session(&server, session_id);

        mock_session_refresh(&server, &refreshed).await;

        Mock::given(method("GET"))
            .and(path("/queue"))
            .and(header("authorization", "Bearer expired-token"))
            .respond_with(ResponseTemplate::new(401).set_body_string("expired"))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/queue"))
            .and(header("authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(202))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_session_client(&server, initial).await;

        let message = client.get_message(0, 8).await.unwrap();

        assert!(message.is_none());

        let current = client.current_session().await;

        assert_eq!(current.session_id, session_id);
        assert_eq!(current.message_queue_access_token, "fresh-token");
    }

    #[tokio::test]
    async fn delete_message_refreshes_expired_session_and_retries() {
        let server = MockServer::start().await;
        let session_id = Uuid::new_v4();

        let initial = initial_session(&server, session_id);
        let refreshed = refreshed_session(&server, session_id);

        mock_session_refresh(&server, &refreshed).await;

        Mock::given(method("DELETE"))
            .and(path("/queue/99"))
            .and(header("authorization", "Bearer expired-token"))
            .respond_with(ResponseTemplate::new(401).set_body_string("expired"))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("DELETE"))
            .and(path("/queue/99"))
            .and(header("authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_session_client(&server, initial).await;

        client.delete_message(99).await.unwrap();

        let current = client.current_session().await;

        assert_eq!(current.session_id, session_id);
        assert_eq!(current.message_queue_access_token, "fresh-token");
    }

    #[tokio::test]
    async fn acquire_jobs_refreshes_expired_session_and_retries() {
        let server = MockServer::start().await;
        let session_id = Uuid::new_v4();

        let initial = initial_session(&server, session_id);
        let refreshed = refreshed_session(&server, session_id);

        mock_session_refresh(&server, &refreshed).await;

        let acquire_path = format!("/{SCALE_SET_ENDPOINT}/{SCALE_SET_ID}/acquirejobs");

        Mock::given(method("POST"))
            .and(path(acquire_path.clone()))
            .and(header("authorization", "Bearer expired-token"))
            .respond_with(ResponseTemplate::new(401).set_body_string("expired"))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path(acquire_path))
            .and(header("authorization", "Bearer fresh-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "count": 2,
                "value": [501, 502]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_session_client(&server, initial).await;

        let acquired = client.acquire_jobs(&[501, 502]).await.unwrap();

        assert_eq!(acquired, vec![501, 502]);

        let current = client.current_session().await;

        assert_eq!(current.session_id, session_id);
        assert_eq!(current.message_queue_access_token, "fresh-token");
    }

    #[tokio::test]
    async fn session_client_can_override_parent_http_options() {
        let server = MockServer::start().await;

        let client = Client::with_personal_access_token(
            PersonalAccessTokenConfig {
                github_config_url: format!("{}/octo-org", server.uri()),
                personal_access_token: "github-pat".to_string(),
                system_info: SystemInfo::default(),
            },
            HttpOptions {
                retry_max: 0,
                ..HttpOptions::default()
            },
        )
        .unwrap();

        *client.inner.admin.write().await = AdminToken {
            token: "admin-token".to_string(),
            authorization_header: "Bearer admin-token".to_string(),
            expires_at: Some(OffsetDateTime::now_utc() + Duration::hours(1)),
            url: server.uri(),
        };

        let created_session = RunnerScaleSetSession {
            session_id: Uuid::new_v4(),
            owner_name: "test-owner".to_string(),
            message_queue_url: format!("{}/queue", server.uri()),
            message_queue_access_token: "queue-token".to_string(),
            ..RunnerScaleSetSession::default()
        };

        Mock::given(method("POST"))
            .and(path(format!(
                "/{SCALE_SET_ENDPOINT}/{SCALE_SET_ID}/sessions"
            )))
            .and(header("authorization", "Bearer admin-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&created_session))
            .expect(1)
            .mount(&server)
            .await;

        let calls = Arc::new(AtomicUsize::new(0));
        let responder_calls = Arc::clone(&calls);

        Mock::given(method("GET"))
            .and(path("/queue"))
            .respond_with(move |_request: &Request| {
                let attempt = responder_calls.fetch_add(1, Ordering::SeqCst);

                match attempt {
                    0 => ResponseTemplate::new(500),
                    _ => ResponseTemplate::new(202),
                }
            })
            .expect(2)
            .mount(&server)
            .await;

        let session_client = client
            .message_session_client_with_http_options(
                SCALE_SET_ID,
                "test-owner",
                HttpOptions {
                    retry_max: 1,
                    retry_wait_max: std::time::Duration::ZERO,
                    ..HttpOptions::default()
                },
            )
            .await
            .unwrap();

        let message = session_client.get_message(0, 8).await.unwrap();

        assert!(message.is_none());

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
