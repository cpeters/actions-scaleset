use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Serialize;
use time::{Duration, OffsetDateTime};

use crate::error::{Error, Result};

/// GitHub App credentials. All fields are required.
#[derive(Debug, Clone)]
pub struct GitHubAppAuth {
    pub client_id: String,
    pub installation_id: i64,
    pub private_key_pem: String,
}

impl GitHubAppAuth {
    pub fn validate(&self) -> Result<()> {
        if self.client_id.is_empty() {
            return Err(Error::message("client ID is required"));
        }
        if self.installation_id == 0 {
            return Err(Error::message("app installation ID is required"));
        }
        if self.private_key_pem.is_empty() {
            return Err(Error::message("app private key is required"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ActionsAuth {
    App(GitHubAppAuth),
    Token(String),
}

impl ActionsAuth {
    pub(crate) fn validate(&self) -> Result<()> {
        match self {
            Self::App(app) => app.validate(),
            Self::Token(t) if t.is_empty() => {
                Err(Error::message("personal access token is required"))
            }
            Self::Token(_) => Ok(()),
        }
    }
}

#[derive(Debug, Serialize)]
struct AppClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

pub(crate) fn create_jwt_for_github_app(app: &GitHubAppAuth) -> Result<String> {
    let issued_at = OffsetDateTime::now_utc() - Duration::seconds(60);
    let expires_at = issued_at + Duration::minutes(9);
    let claims = AppClaims {
        iat: issued_at.unix_timestamp(),
        exp: expires_at.unix_timestamp(),
        iss: app.client_id.clone(),
    };
    let key = EncodingKey::from_rsa_pem(app.private_key_pem.as_bytes())?;
    Ok(jsonwebtoken::encode(
        &Header::new(Algorithm::RS256),
        &claims,
        &key,
    )?)
}

pub(crate) fn jwt_expires_at(token: &str) -> Result<OffsetDateTime> {
    let payload = token.split('.').nth(1).ok_or(Error::JwtMissingExp)?;
    parse_exp_from_payload(payload)
}

fn parse_exp_from_payload(payload_b64: &str) -> Result<OffsetDateTime> {
    let normalized = payload_b64.replace('-', "+").replace('_', "/");
    let mut buf = normalized;
    while buf.len() % 4 != 0 {
        buf.push('=');
    }
    let bytes = decode_b64(&buf)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let exp = value
        .get("exp")
        .and_then(|v| v.as_i64())
        .ok_or(Error::JwtMissingExp)?;
    OffsetDateTime::from_unix_timestamp(exp).map_err(|_| Error::JwtMissingExp)
}

fn decode_b64(input: &str) -> Result<Vec<u8>> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let val = |c: u8| -> u8 {
        if c == b'=' {
            return 0;
        }
        TABLE.iter().position(|&x| x == c).unwrap_or(0) as u8
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < bytes.len() {
        let a = val(bytes[i]);
        let b = val(bytes[i + 1]);
        let c = val(bytes[i + 2]);
        let d = val(bytes[i + 3]);
        out.push((a << 2) | (b >> 4));
        if bytes[i + 2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if bytes[i + 3] != b'=' {
            out.push((c << 6) | d);
        }
        i += 4;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PEM: &str = include_str!("../tests/fixtures/app.pem");

    #[test]
    fn signs_and_reads_exp() {
        let auth = GitHubAppAuth {
            client_id: "iv1.abc".into(),
            installation_id: 1,
            private_key_pem: TEST_PEM.into(),
        };
        let jwt = create_jwt_for_github_app(&auth).expect("sign");
        let exp = jwt_expires_at(&jwt).expect("exp");
        let now = OffsetDateTime::now_utc();
        assert!(exp > now);
        assert!(exp < now + Duration::minutes(12));
    }
}
