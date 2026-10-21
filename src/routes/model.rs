use super::path::canonical_path;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use url::Url;

/// Route fields accepted from the management API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteInput {
    pub name: String,
    pub host: String,
    #[serde(default = "root_path")]
    pub path_prefix: String,
    pub upstream: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub forward_auth_url: Option<String>,
}

fn root_path() -> String {
    "/".into()
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub id: String,
    #[serde(flatten)]
    pub input: RouteInput,
}

impl RouteInput {
    /// Validates every field and normalizes it in place.
    pub fn validate(&mut self) -> Result<()> {
        self.name = self.name.trim().to_string();
        ensure!(
            !self.name.is_empty() && self.name.len() <= 100,
            "name must contain 1–100 characters"
        );
        self.host = normalize_host(&self.host)?;
        self.path_prefix = normalize_prefix(&self.path_prefix)?;
        self.upstream = normalize_upstream(&self.upstream)?;
        self.forward_auth_url = match self.forward_auth_url.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(auth) => Some(endpoint(auth)?.to_string()),
        };
        Ok(())
    }
}

/// Exact DNS hostnames only; wildcards and ports are intentionally unsupported for now.
fn normalize_host(value: &str) -> Result<String> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    ensure!(
        !value.is_empty() && value.len() <= 253,
        "invalid hostname length"
    );
    ensure!(
        value.split('.').all(|label| !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')),
        "use an exact DNS hostname without a port or wildcard"
    );
    Ok(value)
}

fn normalize_prefix(value: &str) -> Result<String> {
    let prefix = value.trim();
    ensure!(
        prefix.starts_with('/')
            && prefix.len() <= 2048
            && !prefix.contains(['?', '#', '%'])
            && !prefix.chars().any(char::is_control),
        "path prefix must start with '/' and contain no query, fragment or percent-encoding"
    );
    canonical_path(prefix).context("path prefix must not contain '.', '..' or '\\'")
}

fn normalize_upstream(value: &str) -> Result<String> {
    let url = endpoint(value)?;
    ensure!(
        url.path() == "/" && url.query().is_none(),
        "upstream must be an origin, e.g. http://127.0.0.1:3000"
    );
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// Parses an HTTP(S) URL without credentials or fragment.
pub(super) fn endpoint(value: &str) -> Result<Url> {
    let url = Url::parse(value)?;
    ensure!(
        matches!(url.scheme(), "http" | "https") && url.host().is_some(),
        "expected an HTTP or HTTPS URL"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
        "URL credentials and fragments are not supported"
    );
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(path: &str) -> RouteInput {
        RouteInput {
            name: " test ".into(),
            host: "App.Local.".into(),
            path_prefix: path.into(),
            upstream: "http://127.0.0.1:9000/".into(),
            enabled: true,
            forward_auth_url: Some("  ".into()),
        }
    }

    #[test]
    fn valid_input_is_normalized() {
        let mut route = input("/api//v1/");
        route.validate().unwrap();
        assert_eq!(route.name, "test");
        assert_eq!(route.host, "app.local");
        assert_eq!(route.path_prefix, "/api/v1");
        assert_eq!(route.upstream, "http://127.0.0.1:9000");
        assert!(route.forward_auth_url.is_none());
    }

    #[test]
    fn invalid_input_is_rejected() {
        for host in ["*.local", "host:80", "-bad.local", "bad..local", ""] {
            assert!(normalize_host(host).is_err(), "{host}");
        }
        for prefix in ["api", "/a/../b", "/a?b", "/%61"] {
            assert!(normalize_prefix(prefix).is_err(), "{prefix}");
        }
        for upstream in [
            "http://user:password@127.0.0.1",
            "file:///etc/passwd",
            "http://127.0.0.1/base",
        ] {
            assert!(normalize_upstream(upstream).is_err(), "{upstream}");
        }
    }
}
