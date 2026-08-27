use std::time::Duration;

use reqwest::{header::RETRY_AFTER, Client as HttpClient, RequestBuilder, Response, StatusCode};
use time::{format_description::well_known::Rfc2822, OffsetDateTime};

use crate::error::{map_response_error, Error, Kind, ResponseErrorContext, Result};
use crate::types::SystemInfo;

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_RETRY_MAX: u32 = 4;
const DEFAULT_RETRY_WAIT_MIN: Duration = Duration::from_secs(1);
const DEFAULT_RETRY_WAIT_MAX: Duration = Duration::from_secs(30);
const DEFAULT_DANGER_ACCEPT_INVALID_CERTS: bool = false;
const RETRY_RESPONSE_DRAIN_LIMIT: usize = 4096;

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
            let Some(request) = builder.try_clone() else {
                return builder.send().await.map_err(Into::into);
            };

            match request.send().await {
                Ok(response) => {
                    if !should_retry_status(response.status()) || attempt + 1 >= attempts {
                        return Ok(response);
                    }

                    let retry_after = retry_after_duration(&response);

                    drain_retry_response(response).await;
                    backoff(attempt, self.options.retry_wait_max, retry_after).await;
                }

                Err(err) => {
                    last_err = Some(err.into());

                    if attempt + 1 >= attempts {
                        break;
                    }

                    backoff(attempt, self.options.retry_wait_max, None).await;
                }
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

fn retry_after_duration(response: &Response) -> Option<Duration> {
    match response.status() {
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {}
        _ => return None,
    }

    let value = response.headers().get(RETRY_AFTER)?.to_str().ok()?;

    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let retry_at = OffsetDateTime::parse(value, &Rfc2822).ok()?;
    let now = OffsetDateTime::now_utc();

    if retry_at <= now {
        return Some(Duration::ZERO);
    }

    Duration::try_from(retry_at - now).ok()
}

async fn drain_retry_response(mut response: Response) {
    let mut bytes_read = 0;

    while bytes_read < RETRY_RESPONSE_DRAIN_LIMIT {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                bytes_read += chunk.len();
            }

            Ok(None) | Err(_) => {
                break;
            }
        }
    }
}

fn backoff_duration(attempt: u32, cap: Duration) -> Duration {
    let multiplier = 2u32.saturating_pow(attempt);

    DEFAULT_RETRY_WAIT_MIN.saturating_mul(multiplier).min(cap)
}

async fn backoff(attempt: u32, cap: Duration, retry_after: Option<Duration>) {
    let delay = match retry_after {
        Some(delay) => delay,
        None => backoff_duration(attempt, cap),
    };

    tokio::time::sleep(delay).await;
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
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let body = resp.bytes().await.unwrap_or_default();

    map_response_error(ResponseErrorContext {
        status,
        activity_id,
        github_request_id,
        content_type: content_type.as_deref(),
        body: &body,
        method,
        url,
        seed,
    })
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
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use super::*;
    use wiremock::{
        matchers::{body_json, method, path},
        Mock, MockServer, Request, ResponseTemplate,
    };

    fn test_transport(retry_max: u32) -> Transport {
        let options = HttpOptions {
            retry_max,
            retry_wait_max: Duration::ZERO,
            ..HttpOptions::default()
        };

        Transport::new(SystemInfo::default(), options).unwrap()
    }

    #[test]
    fn retry_status_matches_upstream_policy() {
        assert!(should_retry_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(should_retry_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(should_retry_status(StatusCode::TOO_MANY_REQUESTS));

        assert!(!should_retry_status(StatusCode::NOT_IMPLEMENTED));
        assert!(!should_retry_status(StatusCode::REQUEST_TIMEOUT));
        assert!(!should_retry_status(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn backoff_matches_upstream_defaults() {
        let cap = Duration::from_secs(30);

        assert_eq!(backoff_duration(0, cap), Duration::from_secs(1));
        assert_eq!(backoff_duration(1, cap), Duration::from_secs(2));
        assert_eq!(backoff_duration(2, cap), Duration::from_secs(4));
        assert_eq!(backoff_duration(3, cap), Duration::from_secs(8));
    }

    #[tokio::test]
    async fn retries_server_errors_until_success() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let responder_calls = Arc::clone(&calls);

        Mock::given(method("GET"))
            .and(path("/retry"))
            .respond_with(move |_request: &Request| {
                let attempt = responder_calls.fetch_add(1, Ordering::SeqCst);

                match attempt {
                    0 | 1 => ResponseTemplate::new(500).set_body_string("temporary failure"),
                    _ => ResponseTemplate::new(200),
                }
            })
            .mount(&server)
            .await;

        let transport = test_transport(4);
        let request = transport.http.get(format!("{}/retry", server.uri()));

        let response = transport.send(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_max_zero_makes_one_attempt() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let responder_calls = Arc::clone(&calls);

        Mock::given(method("GET"))
            .and(path("/retry"))
            .respond_with(move |_request: &Request| {
                responder_calls.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(500)
            })
            .mount(&server)
            .await;

        let transport = test_transport(0);
        let request = transport.http.get(format!("{}/retry", server.uri()));

        let response = transport.send(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_replay_json_request_body() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let responder_calls = Arc::clone(&calls);

        let payload = serde_json::json!({
            "runnerRequestId": 501
        });

        Mock::given(method("POST"))
            .and(path("/retry"))
            .and(body_json(&payload))
            .respond_with(move |_request: &Request| {
                let attempt = responder_calls.fetch_add(1, Ordering::SeqCst);

                match attempt {
                    0 => ResponseTemplate::new(500),
                    _ => ResponseTemplate::new(200),
                }
            })
            .mount(&server)
            .await;

        let transport = test_transport(1);
        let request = transport
            .http
            .post(format!("{}/retry", server.uri()))
            .json(&payload);

        let response = transport.send(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
