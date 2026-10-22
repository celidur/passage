use super::{
    path::canonical_path,
    policy::{MAX_UPSTREAMS, Policy},
};
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
    /// Origins that share the traffic, e.g. `http://10.0.0.5:3000`.
    pub upstreams: Vec<String>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub forward_auth_url: Option<String>,
    #[serde(default)]
    pub policy: Policy,
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
        ensure!(
            (1..=MAX_UPSTREAMS).contains(&self.upstreams.len()),
            "provide between 1 and {MAX_UPSTREAMS} upstreams"
        );
        for upstream in &mut self.upstreams {
            *upstream = normalize_upstream(upstream)?;
        }
        let mut seen = std::collections::HashSet::new();
        ensure!(
            self.upstreams.iter().all(|u| seen.insert(u.as_str())),
            "upstreams must be distinct"
        );
        self.policy.validate()?;
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
pub(crate) fn endpoint(value: &str) -> Result<Url> {
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
            upstreams: vec!["http://127.0.0.1:9000/".into()],
            enabled: true,
            forward_auth_url: Some("  ".into()),
            policy: Policy::default(),
        }
    }

    #[test]
    fn valid_input_is_normalized() {
        let mut route = input("/api//v1/");
        route.validate().unwrap();
        assert_eq!(route.name, "test");
        assert_eq!(route.host, "app.local");
        assert_eq!(route.path_prefix, "/api/v1");
        assert_eq!(route.upstreams, ["http://127.0.0.1:9000"]);
        assert!(route.forward_auth_url.is_none());
    }

    #[test]
    fn upstream_list_and_policy_are_validated() {
        let mut route = input("/");
        route.upstreams.clear();
        assert!(route.validate().is_err(), "at least one upstream");

        let mut route = input("/");
        route.upstreams = vec!["http://a.local:1".into(), "http://a.local:1/".into()];
        assert!(route.validate().is_err(), "duplicates after normalization");

        let mut route = input("/");
        route.upstreams = (0..17).map(|i| format!("http://10.0.0.{i}:80")).collect();
        assert!(route.validate().is_err(), "too many upstreams");

        let mut route = input("/");
        route.policy.timeouts.read_secs = 0;
        assert!(route.validate().is_err());
        let mut route = input("/");
        route.policy.limits.max_connections = Some(0);
        assert!(route.validate().is_err());
        let mut route = input("/");
        route.policy.retries = 6;
        assert!(route.validate().is_err());
        let mut route = input("/");
        let check = crate::routes::policy::ActiveCheck {
            timeout_secs: 30,
            ..Default::default()
        };
        route.policy.health.check = Some(check);
        assert!(route.validate().is_err(), "timeout above interval");
    }

    #[test]
    fn policy_defaults_apply_and_unknown_fields_are_rejected() {
        let route: RouteInput = serde_json::from_str(
            r#"{"name":"a","host":"a.local","upstreams":["http://127.0.0.1:1"],
                "policy":{"timeouts":{"read_secs":5},"health":{"check":{"path":"/up"}}}}"#,
        )
        .unwrap();
        assert_eq!(route.policy.timeouts.read_secs, 5);
        assert_eq!(route.policy.timeouts.connect_secs, 5);
        assert_eq!(route.policy.retries, 2);
        assert_eq!(route.policy.health.check.unwrap().interval_secs, 10);
        assert!(
            serde_json::from_str::<RouteInput>(
                r#"{"name":"a","host":"a.local","upstreams":[],"policy":{"timeout":1}}"#
            )
            .is_err()
        );
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
