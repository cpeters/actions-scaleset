use async_trait::async_trait;
use reqwest::StatusCode;
use tokio::sync::{Mutex, RwLock};
use url::Url;
use uuid::Uuid;

use crate::client::Client;
use crate::error::{Error, Kind, Result};
use crate::http::{expect_status, read_error_body};
use crate::types::{
    parse_runner_scale_set_message, AcquireJobsResponse, RunnerScaleSetMessage,
    RunnerScaleSetMessageResponse, RunnerScaleSetSession, API_VERSION, HEADER_SCALE_SET_MAX_CAPACITY,
    SCALE_SET_ENDPOINT,
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
    fn session(&self) -> RunnerScaleSetSession;
}

pub struct MessageSessionClient {
    inner: Client,
    scale_set_id: i32,
    owner: String,
    session: RwLock<RunnerScaleSetSession>,
    refresh: Mutex<()>,
}

impl MessageSessionClient {
    pub(crate) async fn create(inner: Client, scale_set_id: i32, owner: String) -> Result<Self> {
        let client = Self {
            inner,
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
        let body = serde_json::json!({ "ownerName": self.owner });
        let auth = self.inner.admin_authorization().await?;
        let resp = self
            .inner
            .post_actions_raw(&url, &auth, serde_json::to_vec(&body)?)
            .await?;
        let resp = expect_status(resp, StatusCode::OK, "POST", url.as_str()).await?;
        let created: RunnerScaleSetSession = resp.json().await?;
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
        let t = self.inner.inner.transport.read().await;
        let builder = t
            .http
            .delete(&url)
            .header("Authorization", auth)
            .header("User-Agent", t.user_agent.clone())
            .header("Content-Type", "application/json");
        let resp = t.send(builder).await?;
        expect_status(resp, StatusCode::NO_CONTENT, "DELETE", url.as_str()).await?;
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
        let t = self.inner.inner.transport.read().await;
        let builder = t
            .http
            .patch(&url)
            .header("Authorization", auth)
            .header("User-Agent", t.user_agent.clone())
            .header("Content-Type", "application/json");
        let resp = t.send(builder).await?;
        let resp = expect_status(resp, StatusCode::OK, "PATCH", url.as_str()).await?;
        *self.session.write().await = resp.json().await?;
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
        let t = self.inner.inner.transport.read().await;
        let builder = t
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
            .header("User-Agent", t.user_agent.clone())
            .header(HEADER_SCALE_SET_MAX_CAPACITY, max_capacity.to_string());
        let resp = t.send(builder).await?;
        match resp.status() {
            StatusCode::ACCEPTED => Ok(None),
            StatusCode::OK => {
                let parsed: RunnerScaleSetMessageResponse = resp.json().await?;
                Ok(Some(parse_runner_scale_set_message(parsed)?))
            }
            StatusCode::UNAUTHORIZED => Err(read_error_body(
                resp,
                "GET",
                url.as_str(),
                Some(Kind::MessageQueueTokenExpired),
            )
            .await),
            _ => Err(read_error_body(resp, "GET", url.as_str(), None).await),
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
        let t = self.inner.inner.transport.read().await;
        let builder = t
            .http
            .delete(url.as_str())
            .header("Content-Type", "application/json")
            .header(
                "Authorization",
                format!("Bearer {}", session.message_queue_access_token),
            )
            .header("User-Agent", t.user_agent.clone());
        let resp = t.send(builder).await?;
        match resp.status() {
            StatusCode::NO_CONTENT => Ok(()),
            StatusCode::UNAUTHORIZED => Err(read_error_body(
                resp,
                "DELETE",
                url.as_str(),
                Some(Kind::MessageQueueTokenExpired),
            )
            .await),
            _ => Err(read_error_body(resp, "DELETE", url.as_str(), None).await),
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
        let resp = self
            .inner
            .post_actions_raw(
                &url,
                &format!("Bearer {}", session.message_queue_access_token),
                body,
            )
            .await?;
        match resp.status() {
            StatusCode::UNAUTHORIZED => Err(read_error_body(
                resp,
                "POST",
                url.as_str(),
                Some(Kind::MessageQueueTokenExpired),
            )
            .await),
            StatusCode::OK => {
                let parsed: AcquireJobsResponse = resp.json().await?;
                Ok(parsed.value)
            }
            _ => Err(read_error_body(resp, "POST", url.as_str(), None).await),
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
            Ok(v) => Ok(v),
            Err(err) if err.is_message_queue_token_expired() => {
                self.refresh_session(&session).await?;
                let session = self.current_session().await;
                self.get_message_once(&session, last_message_id, max_capacity)
                    .await
            }
            Err(err) => Err(Error::message(format!("failed to get next message: {err}"))),
        }
    }

    async fn delete_message(&self, message_id: i32) -> Result<()> {
        let session = self.current_session().await;
        match self.delete_message_once(&session, message_id).await {
            Ok(()) => Ok(()),
            Err(err) if err.is_message_queue_token_expired() => {
                self.refresh_session(&session).await?;
                let session = self.current_session().await;
                self.delete_message_once(&session, message_id).await
            }
            Err(err) => Err(Error::message(format!("failed to delete message: {err}"))),
        }
    }

    async fn acquire_jobs(&self, request_ids: &[i64]) -> Result<Vec<i64>> {
        let session = self.current_session().await;
        match self.acquire_jobs_once(&session, request_ids).await {
            Ok(v) => Ok(v),
            Err(err) if err.is_message_queue_token_expired() => {
                self.refresh_session(&session).await?;
                let session = self.current_session().await;
                self.acquire_jobs_once(&session, request_ids).await
            }
            Err(err) => Err(Error::message(format!("failed to acquire jobs: {err}"))),
        }
    }

    fn session(&self) -> RunnerScaleSetSession {
        self.session
            .try_read()
            .map(|g| g.clone())
            .unwrap_or_default()
    }
}
