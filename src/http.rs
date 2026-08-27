use std::time::Duration;

use reqwest::{Client as HttpClient, RequestBuilder, Response, StatusCode};

use crate::error::{Error, Kind, Result};
use crate::types::SystemInfo;

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_RETRY_MAX: u32 = 4;
const DEFAULT_RETRY_WAIT_MAX: Duration = Duration::from_secs(30);
const DEFAULT_DANGER_ACCEPT_INVALID_CERTS: bool = false;

const HEADER_ACTIONS_ACTIVITY_ID: &str = "ActivityId";
const HEADER_GITHUB_REQUEST_ID: &str = "X-GitHub-Request-Id";

#[derive(Debug, Clone)]
pub struct HttpOptions {
    pub timeout: Duration,
    pub retry_max: u32,
    pub retry_wait_max: Duration,
    pub danger_accept_invalid_certs: bool,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_HTTP_TIMEOUT,
            retry_max: DEFAULT_RETRY_MAX,
            retry_wait_max: DEFAULT_RETRY_WAIT_MAX,
            danger_accept_invalid_certs: DEFAULT_DANGER_ACCEPT_INVALID_CERTS,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Transport {
    pub http: HttpClient,
    pub user_agent: String,
    pub options: HttpOptions,
    pub system_info: SystemInfo,
}

impl Transport {
    pub fn new(system_info: SystemInfo, options: HttpOptions) -> Result<Self> {
        let user_agent = user_agent_string(&system_info);
        let http = HttpClient::builder()
            .timeout(options.timeout)
            .danger_accept_invalid_certs(options.danger_accept_invalid_certs)
            .user_agent(&user_agent)
            .build()?;
        Ok(Self {
            http,
            user_agent,
            options,
            system_info,
        })
    }

    pub fn set_system_info(&mut self, info: SystemInfo) {
        self.system_info = info;
        self.user_agent = user_agent_string(&self.system_info);
    }

    pub async fn send(&self, builder: RequestBuilder) -> Result<Response> {
        let mut last_err: Option<Error> = None;
        let attempts = self.options.retry_max.saturating_add(1);

        for attempt in 0..attempts {
            match builder.try_clone() {
                Some(cloned) => match cloned.send().await {
                    Ok(resp) => {
                        if should_retry_status(resp.status()) && attempt + 1 < attempts {
                            backoff(attempt, self.options.retry_wait_max).await;
                            continue;
                        }
                        return Ok(resp);
                    }
                    Err(err) => {
                        last_err = Some(err.into());
                        if attempt + 1 < attempts {
                            backoff(attempt, self.options.retry_wait_max).await;
                            continue;
                        }
                    }
                },
                None => return builder.send().await.map_err(Into::into),
            }
        }
        Err(last_err.unwrap_or_else(|| Error::message("request failed")))
    }
}

fn should_retry_status(status: StatusCode) -> bool {
    match status {
        StatusCode::TOO_MANY_REQUESTS => true,
        StatusCode::NOT_IMPLEMENTED => false,
        status if status.is_server_error() => true,
        _ => false,
    }
}

async fn backoff(attempt: u32, cap: Duration) {
    let millis = (200u64 * 2u64.saturating_pow(attempt)).min(cap.as_millis() as u64);
    tokio::time::sleep(Duration::from_millis(millis)).await;
}

pub(crate) fn user_agent_string(info: &SystemInfo) -> String {
    let payload = serde_json::json!({
        "system": info.system,
        "version": info.version,
        "commit_sha": info.commit_sha,
        "scale_set_id": info.scale_set_id,
        "subsystem": info.subsystem,
        "build_version": env!("CARGO_PKG_VERSION"),
        "build_commit_sha": option_env!("VERGEN_GIT_SHA").unwrap_or("unknown"),
        "kind": "actions-scaleset-rs",
    });
    format!(
        "actions-scaleset-rs/{} {payload}",
        env!("CARGO_PKG_VERSION")
    )
}

pub(crate) async fn read_error_body(
    resp: Response,
    method: &str,
    url: &str,
    seed: Option<Kind>,
) -> Error {
    let status = resp.status();
    let activity_id = header(&resp, HEADER_ACTIONS_ACTIVITY_ID);
    let github_request_id = header(&resp, HEADER_GITHUB_REQUEST_ID);
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = resp.bytes().await.unwrap_or_default();
    crate::error::map_response_error(
        status,
        activity_id,
        github_request_id,
        content_type.as_deref(),
        &body,
        method,
        url,
        seed,
    )
}

pub(crate) async fn expect_status(
    resp: Response,
    expected: StatusCode,
    method: &str,
    url: &str,
) -> Result<Response> {
    if resp.status() == expected {
        return Ok(resp);
    }
    Err(read_error_body(resp, method, url, None).await)
}

fn header(resp: &Response, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_status_matches_upstream_policy() {
        assert!(should_retry_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(should_retry_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(should_retry_status(StatusCode::TOO_MANY_REQUESTS));

        assert!(!should_retry_status(StatusCode::NOT_IMPLEMENTED));
        assert!(!should_retry_status(StatusCode::REQUEST_TIMEOUT));
        assert!(!should_retry_status(StatusCode::BAD_REQUEST));
    }
}
