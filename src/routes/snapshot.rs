use super::{
    model::Route,
    policy::{Limits, Timeouts, UpstreamProtocol},
};
use crate::upstream::{Pool, PoolSpec, Resolver, SystemResolver};
use anyhow::{Context, Result, ensure};
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::task::JoinSet;

/// An enabled and validated route, as used by the proxy.
pub struct CompiledRoute {
    pub id: String,
    pub path_prefix: String,
    pub pool: Arc<Pool>,
    pub protocol: UpstreamProtocol,
    pub timeouts: Timeouts,
    pub limits: Limits,
    pub retries: u32,
    pub forward_auth_url: Option<String>,
}

/// Immutable routing table, swapped atomically on every configuration change.
#[derive(Default)]
pub struct Snapshot {
    /// Routes per normalized host, longest path prefix first.
    by_host: HashMap<String, Vec<Arc<CompiledRoute>>>,
    /// Upstream pool per route id.
    pools: HashMap<String, Arc<Pool>>,
}

impl Snapshot {
    /// Validates every route and resolves enabled upstreams from scratch. A strict compile
    /// fails when a hostname does not resolve; a lenient one (startup) keeps retrying in the
    /// background so a DNS outage cannot stop Passage from starting.
    pub async fn compile(routes: Vec<Route>, strict: bool) -> Result<Self> {
        Self::build(routes, None, strict, Arc::new(SystemResolver)).await
    }

    /// Strictly compiles a changed route set, keeping the pools (and their health and load
    /// state) of routes whose upstream configuration did not change.
    pub async fn recompile(&self, routes: Vec<Route>) -> Result<Self> {
        Self::build(routes, Some(self), true, Arc::new(SystemResolver)).await
    }

    async fn build(
        routes: Vec<Route>,
        previous: Option<&Snapshot>,
        strict: bool,
        resolver: Arc<dyn Resolver>,
    ) -> Result<Self> {
        let mut owners = HashSet::new();
        let mut enabled = Vec::new();
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
            if route.input.enabled {
                enabled.push(route);
            }
        }

        let mut pools: Vec<Option<Arc<Pool>>> = vec![None; enabled.len()];
        let mut connecting = JoinSet::new();
        for (index, route) in enabled.iter().enumerate() {
            let spec = PoolSpec {
                upstreams: route.input.upstreams.clone(),
                balancing: route.input.policy.balancing,
                health: route.input.policy.health.clone(),
            };
            match previous
                .and_then(|p| p.pools.get(&route.id))
                .filter(|pool| *pool.spec() == spec)
            {
                Some(pool) => pools[index] = Some(pool.clone()),
                None => {
                    let resolver = resolver.clone();
                    connecting
                        .spawn(async move { (index, Pool::connect(spec, resolver, strict).await) });
                }
            }
        }
        while let Some(joined) = connecting.join_next().await {
            let (index, pool) = joined.context("upstream resolution task failed")?;
            let name = &enabled[index].input.name;
            pools[index] = Some(Arc::new(pool.with_context(|| format!("route {name:?}"))?));
        }

        let mut by_host: HashMap<String, Vec<Arc<CompiledRoute>>> = HashMap::new();
        let mut by_id = HashMap::new();
        for (route, pool) in enabled.into_iter().zip(pools) {
            let pool = pool.expect("every enabled route has a pool");
            let input = route.input;
            by_id.insert(route.id.clone(), pool.clone());
            by_host
                .entry(input.host)
                .or_default()
                .push(Arc::new(CompiledRoute {
                    id: route.id,
                    path_prefix: input.path_prefix,
                    pool,
                    protocol: input.policy.protocol,
                    timeouts: input.policy.timeouts,
                    limits: input.policy.limits,
                    retries: input.policy.retries,
                    forward_auth_url: input.forward_auth_url,
                }));
        }
        for routes in by_host.values_mut() {
            routes.sort_by_key(|r| Reverse(r.path_prefix.len()));
        }
        Ok(Self {
            by_host,
            pools: by_id,
        })
    }

    pub fn active_routes(&self) -> usize {
        self.by_host.values().map(Vec::len).sum()
    }

    /// Upstream pools of the enabled routes, by route id.
    pub fn pools(&self) -> &HashMap<String, Arc<Pool>> {
        &self.pools
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::{Policy, RouteInput};

    fn route(host: &str, path: &str) -> Route {
        Route {
            id: uuid::Uuid::new_v4().to_string(),
            input: RouteInput {
                name: "test".into(),
                host: host.into(),
                path_prefix: path.into(),
                upstreams: vec!["http://127.0.0.1:9000".into()],
                enabled: true,
                forward_auth_url: None,
                policy: Policy::default(),
            },
        }
    }

    #[tokio::test]
    async fn matches_exact_host_and_longest_segment_prefix() {
        let snapshot = Snapshot::compile(
            vec![route("app.local", "/"), route("app.local", "/api/")],
            true,
        )
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
        let snapshot = Snapshot::compile(vec![disabled], true).await.unwrap();
        assert!(snapshot.find("app.local", "/").is_none());
    }

    #[tokio::test]
    async fn duplicate_host_and_prefix_is_rejected() {
        let routes = vec![route("app.local", "/"), route("app.local", "/")];
        assert!(Snapshot::compile(routes, true).await.is_err());
    }

    #[tokio::test]
    async fn recompile_keeps_pools_of_unchanged_upstreams() {
        let (mut kept, mut changed) = (route("a.local", "/"), route("b.local", "/"));
        let first = Snapshot::compile(vec![kept.clone(), changed.clone()], true)
            .await
            .unwrap();
        kept.input.name = "renamed".into();
        kept.input.policy.timeouts.read_secs = 5;
        changed.input.upstreams = vec!["http://127.0.0.1:9001".into()];
        let second = first
            .recompile(vec![kept.clone(), changed.clone()])
            .await
            .unwrap();
        let pool = |s: &Snapshot, r: &Route| s.pools()[&r.id].clone();
        assert!(Arc::ptr_eq(&pool(&first, &kept), &pool(&second, &kept)));
        assert!(!Arc::ptr_eq(
            &pool(&first, &changed),
            &pool(&second, &changed)
        ));
        assert_eq!(second.find("a.local", "/").unwrap().timeouts.read_secs, 5);
    }
}
