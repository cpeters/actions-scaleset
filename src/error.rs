use std::fmt;

use reqwest::StatusCode;
use thiserror::Error;

/// Sentinel errors matching the Go client's comparable values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Kind {
    #[error("runner not found")]
    RunnerNotFound,
    #[error("runner exists")]
    RunnerExists,
    #[error("job still running")]
    JobStillRunning,
    #[error("message queue token expired")]
    MessageQueueTokenExpired,
    #[error("bad request")]
    BadRequest,
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("conflict")]
    Conflict,
    #[error("invalid github config url")]
    InvalidGitHubConfigUrl,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Kind(#[from] Kind),

    #[error("{kind}: {source}")]
    Classified {
        kind: Kind,
        #[source]
        source: Box<Error>,
    },

    #[error("{0}")]
    Message(String),

    #[error("http {status}: {message}")]
    Http {
        status: u16,
        message: String,
        activity_id: Option<String>,
        github_request_id: Option<String>,
        #[source]
        source: Option<Box<Error>>,
    },

    #[error(transparent)]
    Request(#[from] reqwest::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Jwt(#[from] jsonwebtoken::errors::Error),

    #[error(transparent)]
    Url(#[from] url::ParseError),

    #[error("jwt missing exp claim")]
    JwtMissingExp,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn message(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }

    pub fn is_message_queue_token_expired(&self) -> bool {
        self.has_kind(Kind::MessageQueueTokenExpired)
    }

    pub fn has_kind(&self, kind: Kind) -> bool {
        match self {
            Self::Kind(k) => *k == kind,
            Self::Classified { kind: k, source } => *k == kind || source.has_kind(kind),
            Self::Http {
                source: Some(inner),
                ..
            } => inner.has_kind(kind),
            _ => false,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Message(value.to_string())
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ActionsException {
    #[serde(rename = "typeName", default)]
    type_name: String,
    #[serde(rename = "message", default)]
    message: String,
}

impl fmt::Display for ActionsException {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.type_name, self.message)
    }
}

pub(crate) struct ResponseErrorContext<'a> {
    pub status: StatusCode,
    pub activity_id: Option<String>,
    pub github_request_id: Option<String>,
    pub content_type: Option<&'a str>,
    pub body: &'a [u8],
    pub method: &'a str,
    pub url: &'a str,
    pub seed: Option<Kind>,
}

pub(crate) fn map_response_error(context: ResponseErrorContext<'_>) -> Error {
    let ResponseErrorContext {
        status,
        activity_id,
        github_request_id,
        content_type,
        body,
        method,
        url,
        seed,
    } = context;

    let mut message = format!("request {method} {url} failed (status={status:?}");

    if let Some(id) = &activity_id {
        message.push_str(&format!(", activity_id={id:?}"));
    }

    if let Some(id) = &github_request_id {
        message.push_str(&format!(", github_request_id={id:?}"));
    }

    message.push(')');

    let inner = if body.is_empty() {
        seed.map(Error::from)
            .unwrap_or_else(|| Error::message(format!("{message}: unknown error")))
    } else if let Some(kind) = seed {
        Error::Classified {
            kind,
            source: Box::new(Error::message(format!(
                "{message}: {}",
                String::from_utf8_lossy(body)
            ))),
        }
    } else if content_type.is_some_and(|ct| ct.contains("text/plain")) {
        Error::Message(format!("{message}: {}", String::from_utf8_lossy(body)))
    } else if let Ok(exception) = serde_json::from_slice::<ActionsException>(body) {
        let kind = if exception.type_name.contains("AgentExistsException") {
            Some(Kind::RunnerExists)
        } else if exception.type_name.contains("AgentNotFoundException") {
            Some(Kind::RunnerNotFound)
        } else if exception.type_name.contains("JobStillRunningException") {
            Some(Kind::JobStillRunning)
        } else {
            None
        };

        match kind {
            Some(kind) => Error::Classified {
                kind,
                source: Box::new(Error::message(format!("{message}: {}", exception.message))),
            },
            None => Error::Message(format!("{message}: {exception}")),
        }
    } else {
        Error::Message(format!(
            "{message}: failed to unmarshal error response body: {:?}",
            String::from_utf8_lossy(body)
        ))
    };

    let wrapped_kind = match status {
        StatusCode::BAD_REQUEST => Some(Kind::BadRequest),
        StatusCode::UNAUTHORIZED => Some(Kind::Unauthorized),
        StatusCode::NOT_FOUND => Some(Kind::NotFound),
        StatusCode::CONFLICT => Some(Kind::Conflict),
        _ => None,
    };

    let inner = match wrapped_kind {
        Some(kind) => Error::Classified {
            kind,
            source: Box::new(inner),
        },
        None => inner,
    };

    Error::Http {
        status: status.as_u16(),
        message,
        activity_id,
        github_request_id,
        source: Some(Box::new(inner)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_exception_preserves_specific_and_http_kinds() {
        let err = map_response_error(ResponseErrorContext {
            status: StatusCode::CONFLICT,
            activity_id: None,
            github_request_id: None,
            content_type: Some("application/json"),
            body: br#"{"typeName":"AgentExistsException","message":"runner already exists"}"#,
            method: "POST",
            url: "https://example.test",
            seed: None,
        });

        assert!(err.has_kind(Kind::RunnerExists));
        assert!(err.has_kind(Kind::Conflict));
    }

    #[test]
    fn token_expired_preserves_token_and_http_kinds() {
        let err = map_response_error(ResponseErrorContext {
            status: StatusCode::UNAUTHORIZED,
            activity_id: None,
            github_request_id: None,
            content_type: Some("text/plain"),
            body: b"token expired",
            method: "GET",
            url: "https://example.test",
            seed: Some(Kind::MessageQueueTokenExpired),
        });

        assert!(err.has_kind(Kind::MessageQueueTokenExpired));
        assert!(err.has_kind(Kind::Unauthorized));
    }
}
