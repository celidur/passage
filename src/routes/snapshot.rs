use super::model::{Route, endpoint};
use anyhow::{Context, Result, ensure};
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use url::Host;

const DNS_TIMEOUT: Duration = Duration::from_secs(5);

/// An enabled, validated and DNS-resolved route, as used by the proxy.
#[derive(Debug)]
pub struct CompiledRoute {
    pub id: String,
    pub path_prefix: String,
    pub address: SocketAddr,
    pub tls: bool,
    pub sni: String,
    /// Host header sent upstream (the upstream origin's authority).
    pub upstream_host: String,
    pub forward_auth_url: Option<String>,
}

/// Immutable routing table, swapped atomically on every configuration change.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// Routes per normalized host, longest path prefix first.
    by_host: HashMap<String, Vec<Arc<CompiledRoute>>>,
}

impl Snapshot {
    /// Validates every route and resolves enabled upstreams. Fails on the first invalid route.
    pub async fn compile(routes: Vec<Route>) -> Result<Self> {
        let mut owners = HashSet::new();
        let mut by_host: HashMap<String, Vec<Arc<CompiledRoute>>> = HashMap::new();
        for mut route in routes {
            let name = route.input.name.clone();
            route
                .input
                .validate()
                .with_context(|| format!("route {name:?}"))?;
            let (host, prefix) = (&route.input.host, &route.input.path_prefix);
            ensure!(
                owners.insert((host.clone(), prefix.clone())),
                "{host}{prefix} is already used by another route"
            );
            if !route.input.enabled {
                continue;
            }
            let host = host.clone();
            let compiled = compile_route(route)
                .await
                .with_context(|| format!("route {name:?}"))?;
            by_host.entry(host).or_default().push(Arc::new(compiled));
        }
        for routes in by_host.values_mut() {
            routes.sort_by_key(|r| Reverse(r.path_prefix.len()));
        }
        Ok(Self { by_host })
    }

    pub fn active_routes(&self) -> usize {
        self.by_host.values().map(Vec::len).sum()
    }

    /// Finds the route for a Host header value and a path from [`super::canonical_path`].
    pub fn find(&self, authority: &str, path: &str) -> Option<Arc<CompiledRoute>> {
        let authority = authority.parse::<http::uri::Authority>().ok()?;
        let host = authority.host().trim_end_matches('.').to_ascii_lowercase();
        self.by_host
            .get(&host)?
            .iter()
            .find(|r| matches_prefix(&r.path_prefix, path))
            .cloned()
    }
}

/// Segment-aware: `/api` matches `/api` and `/api/x`, never `/apiculture`.
fn matches_prefix(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

async fn compile_route(route: Route) -> Result<CompiledRoute> {
    let input = route.input;
    let url = endpoint(&input.upstream)?;
    let sni = url
        .host_str()
        .context("upstream has no host")?
        .trim_matches(['[', ']'])
        .to_string();
    let port = url
        .port_or_known_default()
        .context("upstream has no port")?;
    let address = tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host((sni.as_str(), port)))
        .await
        .context("upstream DNS lookup timed out")?
        .context("cannot resolve upstream")?
        .next()
        .context("upstream resolved to no addresses")?;
    let authority = match url.host() {
        Some(Host::Ipv6(ip)) => format!("[{ip}]"),
        _ => sni.clone(),
    };
    let upstream_host = match url.port() {
        Some(port) => format!("{authority}:{port}"),
        None => authority,
    };
    Ok(CompiledRoute {
        id: route.id,
        path_prefix: input.path_prefix,
        address,
        tls: url.scheme() == "https",
        sni,
        upstream_host,
        forward_auth_url: input.forward_auth_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::RouteInput;

    fn route(host: &str, path: &str) -> Route {
        Route {
            id: uuid::Uuid::new_v4().to_string(),
            input: RouteInput {
                name: "test".into(),
                host: host.into(),
                path_prefix: path.into(),
                upstream: "http://127.0.0.1:9000".into(),
                enabled: true,
                forward_auth_url: None,
            },
        }
    }

    #[tokio::test]
    async fn matches_exact_host_and_longest_segment_prefix() {
        let snapshot =
            Snapshot::compile(vec![route("app.local", "/"), route("app.local", "/api/")])
                .await
                .unwrap();
        let prefix = |host, path| snapshot.find(host, path).unwrap().path_prefix.clone();
        assert_eq!(prefix("APP.LOCAL:80", "/api/users"), "/api");
        assert_eq!(prefix("app.local", "/api"), "/api");
        assert_eq!(prefix("app.local", "/apiculture"), "/");
        assert!(snapshot.find("evilapp.local", "/").is_none());
        assert_eq!(snapshot.active_routes(), 2);
    }

    #[tokio::test]
    async fn disabled_routes_are_not_served() {
        let mut disabled = route("app.local", "/");
        disabled.input.enabled = false;
        let snapshot = Snapshot::compile(vec![disabled]).await.unwrap();
        assert!(snapshot.find("app.local", "/").is_none());
    }

    #[tokio::test]
    async fn duplicate_host_and_prefix_is_rejected() {
        let routes = vec![route("app.local", "/"), route("app.local", "/")];
        assert!(Snapshot::compile(routes).await.is_err());
    }
}
