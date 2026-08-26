use url::Url;

use crate::error::{Error, Kind, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHubScope {
    Enterprise,
    Organization,
    Repository,
}

#[derive(Debug, Clone)]
pub struct GitHubConfig {
    pub config_url: Url,
    pub scope: GitHubScope,
    pub enterprise: String,
    pub organization: String,
    pub repository: String,
    pub is_hosted: bool,
}

impl GitHubConfig {
    pub fn parse(input: &str) -> Result<Self> {
        let trimmed = input.trim_matches('/');
        let url = Url::parse(trimmed)?;
        let is_hosted = is_hosted_github_url(&url);

        let path_parts: Vec<String> = url
            .path()
            .trim_matches('/')
            .split('/')
            .filter(|p| !p.is_empty())
            .map(|s| s.to_string())
            .collect();

        let invalid = || {
            Error::Message(format!(
                "{:?}: {}",
                url.as_str(),
                Kind::InvalidGitHubConfigUrl
            ))
        };

        match path_parts.as_slice() {
            [org] if !org.is_empty() => Ok(Self {
                config_url: url,
                scope: GitHubScope::Organization,
                enterprise: String::new(),
                organization: org.clone(),
                repository: String::new(),
                is_hosted,
            }),
            [first, second] if first.eq_ignore_ascii_case("enterprises") => Ok(Self {
                config_url: url,
                scope: GitHubScope::Enterprise,
                enterprise: second.clone(),
                organization: String::new(),
                repository: String::new(),
                is_hosted,
            }),
            [org, repo] => Ok(Self {
                config_url: url,
                scope: GitHubScope::Repository,
                enterprise: String::new(),
                organization: org.clone(),
                repository: repo.clone(),
                is_hosted,
            }),
            _ => Err(invalid()),
        }
    }

    pub fn github_api_url(&self, path: &str) -> Url {
        let mut result = self.config_url.clone();
        result.set_path("");
        result.set_query(None);
        result.set_fragment(None);

        if self.is_hosted {
            let host = self.config_url.host_str().unwrap_or_default();
            let api_host = if host.eq_ignore_ascii_case("www.github.com") {
                "api.github.com"
            } else {
                // api.github.com / api.github.localhost / api.*.ghe.com
                // Keep simple: prefix api. for github.com and github.localhost
                if host.eq_ignore_ascii_case("github.com")
                    || host.eq_ignore_ascii_case("github.localhost")
                {
                    // "api." + host
                    // but for github.com that's api.github.com
                }
                return hosted_api_url(host, path);
            };
            result.set_host(Some(api_host)).ok();
            result.set_path(path);
        } else {
            result.set_path("/api/v3");
            let joined = join_url_path(result.as_str(), path);
            result = Url::parse(&joined).unwrap_or(result);
        }

        result
    }

    pub fn registration_token_path(&self) -> Result<String> {
        match self.scope {
            GitHubScope::Organization => Ok(format!(
                "/orgs/{}/actions/runners/registration-token",
                self.organization
            )),
            GitHubScope::Enterprise => Ok(format!(
                "/enterprises/{}/actions/runners/registration-token",
                self.enterprise
            )),
            GitHubScope::Repository => Ok(format!(
                "/repos/{}/{}/actions/runners/registration-token",
                self.organization, self.repository
            )),
        }
    }
}

fn hosted_api_url(host: &str, path: &str) -> Url {
    let api_host =
        if host.eq_ignore_ascii_case("www.github.com") || host.eq_ignore_ascii_case("github.com") {
            "api.github.com".to_string()
        } else if host.eq_ignore_ascii_case("github.localhost") {
            "api.github.localhost".to_string()
        } else {
            format!("api.{host}")
        };
    let joined = join_url_path(&format!("https://{api_host}"), path);
    Url::parse(&joined).expect("hosted API URL")
}

pub fn is_hosted_github_url(url: &Url) -> bool {
    if std::env::var_os("GITHUB_ACTIONS_FORCE_GHES").is_some() {
        return false;
    }
    let host = url.host_str().unwrap_or_default();
    host.eq_ignore_ascii_case("github.com")
        || host.eq_ignore_ascii_case("www.github.com")
        || host.eq_ignore_ascii_case("github.localhost")
        || host.to_ascii_lowercase().ends_with(".ghe.com")
}

pub(crate) fn join_url_path(base: &str, path: &str) -> String {
    if base.is_empty() {
        if path.is_empty() {
            return String::new();
        }
        if path.starts_with('/') {
            return path.to_string();
        }
        return format!("/{path}");
    }
    if path.is_empty() {
        return base.trim_end_matches('/').to_string();
    }
    if path.starts_with('/') {
        format!("{}{path}", base.trim_end_matches('/'))
    } else {
        format!("{}/{path}", base.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_org_repo_and_enterprise() {
        let org = GitHubConfig::parse("https://github.com/octo-org").unwrap();
        assert_eq!(org.scope, GitHubScope::Organization);
        assert_eq!(org.organization, "octo-org");
        assert!(org.is_hosted);
        assert_eq!(
            org.github_api_url("/zen").as_str(),
            "https://api.github.com/zen"
        );

        let repo = GitHubConfig::parse("https://github.com/octo-org/octo-repo").unwrap();
        assert_eq!(repo.scope, GitHubScope::Repository);
        assert_eq!(
            repo.registration_token_path().unwrap(),
            "/repos/octo-org/octo-repo/actions/runners/registration-token"
        );

        let ent = GitHubConfig::parse("https://github.com/enterprises/octo").unwrap();
        assert_eq!(ent.scope, GitHubScope::Enterprise);
        assert_eq!(
            ent.registration_token_path().unwrap(),
            "/enterprises/octo/actions/runners/registration-token"
        );
    }

    #[test]
    fn rejects_invalid_urls() {
        assert!(GitHubConfig::parse("https://github.com/").is_err());
        assert!(GitHubConfig::parse("https://github.com/a/b/c").is_err());
    }

    #[test]
    fn ghes_uses_api_v3() {
        let cfg = GitHubConfig::parse("https://ghe.example.com/octo-org").unwrap();
        assert!(!cfg.is_hosted);
        let api = cfg.github_api_url("/zen");
        assert!(api.as_str().contains("/api/v3/zen"));
        assert_eq!(api.host_str(), Some("ghe.example.com"));
    }

    #[test]
    fn ghes_preserves_custom_port() {
        let cfg = GitHubConfig::parse("https://ghe.example.com:8443/octo-org").unwrap();
        let api = cfg.github_api_url("/zen");

        assert_eq!(api.as_str(), "https://ghe.example.com:8443/api/v3/zen");
        assert_eq!(api.port(), Some(8443));
    }
}
