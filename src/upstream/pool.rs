//! A route's set of upstream backends: selection, passive health and DNS refresh.

use super::{
    check,
    resolver::{Resolver, resolve_all},
};
use crate::routes::{
    Balancing, endpoint,
    policy::{ActiveCheck, Health},
};
use anyhow::{Context, Result, bail};
use arc_swap::ArcSwap;
use serde::Serialize;
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        Arc,
        atomic::{
            AtomicBool, AtomicU32, AtomicU64, AtomicUsize,
            Ordering::{AcqRel, Relaxed, Release},
        },
    },
    time::{Duration, Instant},
};
use tokio::task::JoinSet;
use url::Host;

/// Addresses kept per hostname; a name returning more is truncated deterministically.
const MAX_ADDRESSES: usize = 16;
/// Retry delay while a hostname has not resolved yet.
const UNRESOLVED_RETRY: Duration = Duration::from_secs(2);

type Lookup = std::io::Result<Vec<SocketAddr>>;
/// Upstreams whose lookup failed, with the reason.
type Failures = Vec<(Arc<Origin>, String)>;

/// One configured upstream URL.
#[derive(Debug)]
pub struct Origin {
    pub upstream: String,
    pub tls: bool,
    /// Host name or IP used for TLS verification.
    pub sni: String,
    /// Authority sent as the `Host` header.
    pub host_header: String,
    pub port: u16,
    /// Set for IP literals, which never need resolving.
    pub literal: Option<IpAddr>,
}

impl Origin {
    pub fn parse(upstream: &str) -> Result<Self> {
        let url = endpoint(upstream)?;
        let sni = url
            .host_str()
            .context("upstream has no host")?
            .trim_matches(['[', ']'])
            .to_string();
        let port = url
            .port_or_known_default()
            .context("upstream has no port")?;
        let literal = match url.host() {
            Some(Host::Ipv4(ip)) => Some(IpAddr::V4(ip)),
            Some(Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
            _ => None,
        };
        let authority = match url.host() {
            Some(Host::Ipv6(ip)) => format!("[{ip}]"),
            _ => sni.clone(),
        };
        Ok(Self {
            upstream: upstream.to_string(),
            tls: url.scheme() == "https",
            sni,
            host_header: match url.port() {
                Some(port) => format!("{authority}:{port}"),
                None => authority,
            },
            port,
            literal,
        })
    }
}

/// A resolved address of an upstream, with its health and load.
#[derive(Debug)]
pub struct Backend {
    pub origin: Arc<Origin>,
    pub address: SocketAddr,
    in_flight: AtomicU32,
    /// Consecutive connection failures.
    failures: AtomicU32,
    /// Skipped until this many milliseconds after the pool epoch; 0 when not set aside.
    down_until: AtomicU64,
    check_down: AtomicBool,
    /// Consecutive check results that disagree with the current check state.
    check_streak: AtomicU32,
}

impl Backend {
    fn new(origin: Arc<Origin>, address: SocketAddr) -> Self {
        Self {
            origin,
            address,
            in_flight: AtomicU32::new(0),
            failures: AtomicU32::new(0),
            down_until: AtomicU64::new(0),
            check_down: AtomicBool::new(false),
            check_streak: AtomicU32::new(0),
        }
    }

    pub fn available(&self, now: u64) -> bool {
        !self.check_down.load(Relaxed) && self.down_until.load(Relaxed) <= now
    }

    /// Returns true when this failure sets the backend aside.
    fn connect_failed(&self, health: &Health, now: u64) -> bool {
        let failures = self.failures.fetch_add(1, Relaxed) + 1;
        if failures < health.max_failures {
            return false;
        }
        let until = now + health.cooldown_secs * 1000;
        self.down_until.swap(until, Relaxed) <= now
    }

    fn responded(&self) {
        self.failures.store(0, Relaxed);
        self.down_until.store(0, Relaxed);
    }

    /// Records an active check result; returns the new state (`true` = healthy) when it flips.
    pub fn check_result(&self, ok: bool, check: &ActiveCheck) -> Option<bool> {
        let down = self.check_down.load(Relaxed);
        if ok != down {
            self.check_streak.store(0, Relaxed);
            return None;
        }
        let threshold = if down {
            check.healthy_threshold
        } else {
            check.unhealthy_threshold
        };
        if self.check_streak.fetch_add(1, Relaxed) + 1 < threshold {
            return None;
        }
        self.check_streak.store(0, Relaxed);
        self.check_down.store(!down, Relaxed);
        Some(down)
    }
}

/// Counts a request against a backend for the duration of the lease.
pub struct Lease(Arc<Backend>);

impl Lease {
    pub fn backend(&self) -> &Arc<Backend> {
        &self.0
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Relaxed);
    }
}

/// Counts a request against the route for the duration of the slot.
pub struct Slot(Arc<Pool>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Relaxed);
    }
}

/// What makes two pools interchangeable; a configuration change that keeps the spec keeps the pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSpec {
    pub upstreams: Vec<String>,
    pub balancing: Balancing,
    pub health: Health,
}

#[derive(Debug, Serialize)]
pub struct BackendStatus {
    pub upstream: String,
    pub address: SocketAddr,
    pub healthy: bool,
    pub in_flight: u32,
}

pub struct Pool {
    spec: PoolSpec,
    origins: Vec<Arc<Origin>>,
    backends: ArcSwap<Vec<Arc<Backend>>>,
    resolver: Arc<dyn Resolver>,
    cursor: AtomicUsize,
    /// Requests currently admitted for the route.
    active: AtomicU32,
    epoch: Instant,
    next_refresh: AtomicU64,
    next_check: AtomicU64,
    refreshing: AtomicBool,
    checking: AtomicBool,
}

impl Pool {
    /// Resolves every upstream. A strict pool fails if any hostname does not resolve; a lenient
    /// one starts without it and keeps retrying, so a DNS outage cannot block startup.
    pub async fn connect(
        spec: PoolSpec,
        resolver: Arc<dyn Resolver>,
        strict: bool,
    ) -> Result<Self> {
        let origins = spec
            .upstreams
            .iter()
            .map(|upstream| Origin::parse(upstream).map(Arc::new))
            .collect::<Result<Vec<_>>>()?;
        let pool = Self {
            spec,
            origins,
            backends: ArcSwap::from_pointee(Vec::new()),
            resolver,
            cursor: AtomicUsize::new(0),
            active: AtomicU32::new(0),
            epoch: Instant::now(),
            next_refresh: AtomicU64::new(0),
            next_check: AtomicU64::new(0),
            refreshing: AtomicBool::new(false),
            checking: AtomicBool::new(false),
        };
        let (backends, failures) = pool.assemble(pool.lookup().await);
        if let Some((origin, error)) = failures.first() {
            if strict {
                bail!("cannot resolve upstream {}: {error}", origin.upstream);
            }
            for (origin, error) in &failures {
                tracing::warn!(upstream = %origin.upstream, %error, "upstream does not resolve yet");
            }
        }
        pool.backends.store(Arc::new(backends));
        Ok(pool)
    }

    pub fn spec(&self) -> &PoolSpec {
        &self.spec
    }

    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Resolves every origin; IP literals need no lookup. Results follow `self.origins`.
    async fn lookup(&self) -> Vec<Lookup> {
        let names = self
            .origins
            .iter()
            .filter(|o| o.literal.is_none())
            .map(|o| (o.sni.clone(), o.port))
            .collect();
        let mut resolved = resolve_all(&self.resolver, names).await.into_iter();
        self.origins
            .iter()
            .map(|origin| match origin.literal {
                Some(ip) => Ok(vec![SocketAddr::new(ip, origin.port)]),
                None => resolved.next().expect("one result per hostname"),
            })
            .collect()
    }

    /// Builds the backend list from fresh lookups, reusing backends whose address is unchanged
    /// so that their health and load survive. A failed lookup keeps the previous addresses.
    fn assemble(&self, results: Vec<Lookup>) -> (Vec<Arc<Backend>>, Failures) {
        let current = self.backends.load();
        let existing = |origin: &Arc<Origin>| -> Vec<Arc<Backend>> {
            current
                .iter()
                .filter(|b| Arc::ptr_eq(&b.origin, origin))
                .cloned()
                .collect()
        };
        let mut backends = Vec::new();
        let mut failures = Vec::new();
        for (origin, result) in self.origins.iter().zip(results) {
            let mut addresses = match result {
                Ok(addresses) if !addresses.is_empty() => addresses,
                Ok(_) => {
                    failures.push((origin.clone(), "no addresses returned".to_string()));
                    backends.extend(existing(origin));
                    continue;
                }
                Err(error) => {
                    failures.push((origin.clone(), error.to_string()));
                    backends.extend(existing(origin));
                    continue;
                }
            };
            addresses.sort();
            addresses.dedup();
            addresses.truncate(MAX_ADDRESSES);
            let reusable = existing(origin);
            for address in addresses {
                let backend = reusable.iter().find(|b| b.address == address).cloned();
                backends.push(
                    backend.unwrap_or_else(|| Arc::new(Backend::new(origin.clone(), address))),
                );
            }
        }
        (backends, failures)
    }

    /// Re-resolves hostnames and publishes the new backend list when it changed.
    pub async fn refresh_dns(&self) {
        let results = self.lookup().await;
        let (backends, failures) = self.assemble(results);
        for (origin, error) in &failures {
            tracing::warn!(
                upstream = %origin.upstream,
                %error,
                "DNS refresh failed; keeping previous addresses"
            );
        }
        let current = self.backends.load();
        let unchanged = current.len() == backends.len()
            && current
                .iter()
                .zip(&backends)
                .all(|(a, b)| Arc::ptr_eq(a, b));
        if unchanged {
            return;
        }
        tracing::info!(
            upstreams = ?backends.iter().map(|b| b.address).collect::<Vec<_>>(),
            "upstream addresses changed"
        );
        self.backends.store(Arc::new(backends));
    }

    /// True when some hostname currently has no address.
    fn unresolved(&self) -> bool {
        let backends = self.backends.load();
        self.origins
            .iter()
            .any(|o| !backends.iter().any(|b| Arc::ptr_eq(&b.origin, o)))
    }

    /// Chooses a backend that has not been tried yet. Healthy backends are preferred; when none
    /// is healthy the pool is tried anyway, so a wrong health verdict cannot black-hole a route.
    pub fn lease(&self, tried: &[SocketAddr]) -> Option<Lease> {
        let backend = self.pick(tried, self.now())?;
        backend.in_flight.fetch_add(1, Relaxed);
        Some(Lease(backend))
    }

    fn pick(&self, tried: &[SocketAddr], now: u64) -> Option<Arc<Backend>> {
        let backends = self.backends.load();
        let count = backends.len();
        if count == 0 {
            return None;
        }
        let cursor = self.cursor.fetch_add(1, Relaxed);
        let select = |only_available: bool| {
            let eligible = || {
                backends.iter().enumerate().filter(move |(_, b)| {
                    !tried.contains(&b.address) && (!only_available || b.available(now))
                })
            };
            match self.spec.balancing {
                Balancing::RoundRobin => {
                    let position = cursor.checked_rem(eligible().count())?;
                    eligible().nth(position).map(|(_, b)| b)
                }
                Balancing::LeastConnections => eligible()
                    .min_by_key(|(i, b)| {
                        (
                            b.in_flight.load(Relaxed),
                            (i + count - cursor % count) % count,
                        )
                    })
                    .map(|(_, b)| b),
            }
        };
        select(true)
            .or_else(|| {
                if tried.is_empty() {
                    select(false)
                } else {
                    None
                }
            })
            .cloned()
    }

    /// True when a retry after a failure on `tried` has somewhere healthy to go.
    pub fn has_candidate(&self, tried: &[SocketAddr]) -> bool {
        let now = self.now();
        self.backends
            .load()
            .iter()
            .any(|b| !tried.contains(&b.address) && b.available(now))
    }

    pub fn report_connect_failure(&self, backend: &Backend) {
        if backend.connect_failed(&self.spec.health, self.now()) {
            tracing::warn!(
                upstream = %backend.origin.upstream,
                address = %backend.address,
                cooldown_secs = self.spec.health.cooldown_secs,
                "upstream set aside after repeated connection failures"
            );
        }
    }

    pub fn report_response(&self, backend: &Backend) {
        backend.responded();
    }

    /// Admits a request unless the route is at its concurrency limit.
    pub fn admit(self: &Arc<Self>, limit: Option<u32>) -> Option<Slot> {
        let previous = self.active.fetch_add(1, Relaxed);
        if limit.is_some_and(|max| previous >= max) {
            self.active.fetch_sub(1, Relaxed);
            return None;
        }
        Some(Slot(self.clone()))
    }

    pub fn active(&self) -> u32 {
        self.active.load(Relaxed)
    }

    pub fn status(&self) -> Vec<BackendStatus> {
        let now = self.now();
        self.backends
            .load()
            .iter()
            .map(|b| BackendStatus {
                upstream: b.origin.upstream.clone(),
                address: b.address,
                healthy: b.available(now),
                in_flight: b.in_flight.load(Relaxed),
            })
            .collect()
    }

    /// Starts whichever periodic jobs are due. Called every second by the maintenance service.
    pub fn tick(self: &Arc<Self>, dns_interval: Duration) {
        let now = self.now();
        let dynamic = self.origins.iter().any(|o| o.literal.is_none());
        if dynamic && now >= self.next_refresh.load(Relaxed) && !self.refreshing.swap(true, AcqRel)
        {
            let pool = self.clone();
            tokio::spawn(async move {
                pool.refresh_dns().await;
                let wait = if pool.unresolved() {
                    UNRESOLVED_RETRY
                } else {
                    dns_interval
                };
                pool.next_refresh
                    .store(pool.now() + wait.as_millis() as u64, Relaxed);
                pool.refreshing.store(false, Release);
            });
        }
        if let Some(check) = &self.spec.health.check
            && now >= self.next_check.load(Relaxed)
            && !self.checking.swap(true, AcqRel)
        {
            let (pool, interval) = (self.clone(), check.interval_secs * 1000);
            tokio::spawn(async move {
                pool.run_checks().await;
                pool.next_check.store(pool.now() + interval, Relaxed);
                pool.checking.store(false, Release);
            });
        }
    }

    /// Probes every backend concurrently and applies the results.
    pub async fn run_checks(&self) {
        let Some(check) = self.spec.health.check.clone() else {
            return;
        };
        let mut probes = JoinSet::new();
        for backend in self.backends.load().iter().cloned() {
            let check = check.clone();
            probes.spawn(async move {
                let ok = check::probe(&backend, &check).await;
                if let Some(healthy) = backend.check_result(ok, &check) {
                    tracing::warn!(
                        upstream = %backend.origin.upstream,
                        address = %backend.address,
                        healthy,
                        "upstream health check changed state"
                    );
                }
            });
        }
        while probes.join_next().await.is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::{collections::HashMap, io, sync::Mutex};

    #[derive(Default)]
    struct FakeResolver(Mutex<HashMap<String, Vec<IpAddr>>>);

    impl FakeResolver {
        fn set(&self, host: &str, ips: &[&str]) {
            let ips = ips.iter().map(|ip| ip.parse().unwrap()).collect();
            self.0.lock().unwrap().insert(host.into(), ips);
        }

        fn fail(&self, host: &str) {
            self.0.lock().unwrap().remove(host);
        }
    }

    #[async_trait]
    impl Resolver for FakeResolver {
        async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
            match self.0.lock().unwrap().get(host) {
                Some(ips) => Ok(ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect()),
                None => Err(io::Error::other("NXDOMAIN")),
            }
        }
    }

    fn spec(upstreams: &[&str], balancing: Balancing) -> PoolSpec {
        PoolSpec {
            upstreams: upstreams.iter().map(|u| u.to_string()).collect(),
            balancing,
            health: Health::default(),
        }
    }

    async fn pool(upstreams: &[&str], balancing: Balancing) -> Pool {
        let resolver = Arc::new(FakeResolver::default());
        Pool::connect(spec(upstreams, balancing), resolver, true)
            .await
            .unwrap()
    }

    fn port(backend: &Option<Arc<Backend>>) -> u16 {
        backend.as_ref().unwrap().address.port()
    }

    #[tokio::test]
    async fn round_robin_shares_traffic_evenly() {
        let pool = pool(
            &[
                "http://10.0.0.1:1",
                "http://10.0.0.2:2",
                "http://10.0.0.3:3",
            ],
            Balancing::RoundRobin,
        )
        .await;
        let mut seen = [0; 4];
        for _ in 0..30 {
            seen[port(&pool.pick(&[], 0)) as usize] += 1;
        }
        assert_eq!(seen, [0, 10, 10, 10]);
    }

    #[tokio::test]
    async fn failing_backend_is_set_aside_then_retried_after_cooldown() {
        let pool = pool(
            &["http://10.0.0.1:1", "http://10.0.0.2:2"],
            Balancing::RoundRobin,
        )
        .await;
        let bad = pool.backends.load()[0].clone();
        for _ in 0..2 {
            assert!(!bad.connect_failed(&pool.spec.health, 1_000));
        }
        assert!(
            bad.connect_failed(&pool.spec.health, 1_000),
            "third failure ejects"
        );
        for _ in 0..6 {
            assert_eq!(port(&pool.pick(&[], 2_000)), 2);
        }
        assert!(!pool.has_candidate(&[bad.address, pool.backends.load()[1].address]));
        // The cooldown (10 s) has elapsed: the backend is tried again, and one success clears it.
        let after = 1_000 + 10_000;
        assert!((0..2).any(|_| port(&pool.pick(&[], after)) == 1));
        bad.responded();
        assert!(bad.available(after) && bad.failures.load(Relaxed) == 0);
    }

    #[tokio::test]
    async fn when_nothing_is_healthy_every_backend_is_still_tried_once() {
        let pool = pool(
            &["http://10.0.0.1:1", "http://10.0.0.2:2"],
            Balancing::RoundRobin,
        )
        .await;
        for backend in pool.backends.load().iter() {
            for _ in 0..3 {
                backend.connect_failed(&pool.spec.health, 0);
            }
        }
        let first = pool.pick(&[], 1).expect("panic mode picks anyway");
        assert!(
            pool.pick(&[first.address], 1).is_none(),
            "retries need a healthy candidate"
        );
    }

    #[tokio::test]
    async fn least_connections_prefers_the_idle_backend() {
        let pool = pool(
            &["http://10.0.0.1:1", "http://10.0.0.2:2"],
            Balancing::LeastConnections,
        )
        .await;
        let busy = pool.lease(&[]).unwrap();
        let busy_port = busy.backend().address.port();
        for _ in 0..5 {
            let lease = pool.lease(&[]).unwrap();
            assert_ne!(lease.backend().address.port(), busy_port);
        }
        drop(busy);
        assert_eq!(pool.status().iter().map(|b| b.in_flight).sum::<u32>(), 0);
    }

    #[tokio::test]
    async fn dns_refresh_follows_address_changes_and_keeps_health() {
        let resolver = Arc::new(FakeResolver::default());
        resolver.set("app.test", &["10.0.0.1"]);
        let pool = Pool::connect(
            spec(&["http://app.test:8080"], Balancing::RoundRobin),
            resolver.clone(),
            true,
        )
        .await
        .unwrap();
        let first = pool.backends.load()[0].clone();

        resolver.set("app.test", &["10.0.0.2", "10.0.0.1"]);
        pool.refresh_dns().await;
        let addresses: Vec<_> = pool
            .status()
            .iter()
            .map(|b| b.address.to_string())
            .collect();
        assert_eq!(addresses, ["10.0.0.1:8080", "10.0.0.2:8080"]);
        assert!(
            Arc::ptr_eq(&first, &pool.backends.load()[0]),
            "unchanged address keeps its state"
        );

        resolver.fail("app.test");
        pool.refresh_dns().await;
        assert_eq!(
            pool.status().len(),
            2,
            "a failed lookup keeps the last known addresses"
        );

        resolver.set("app.test", &["10.0.0.9"]);
        pool.refresh_dns().await;
        assert_eq!(pool.status()[0].address.to_string(), "10.0.0.9:8080");
        assert_eq!(pool.status().len(), 1);
    }

    #[tokio::test]
    async fn strict_connect_rejects_unresolvable_names_but_lenient_recovers() {
        let resolver = Arc::new(FakeResolver::default());
        let upstreams = spec(
            &["http://late.test:80", "http://10.0.0.1:80"],
            Balancing::RoundRobin,
        );
        let error = Pool::connect(upstreams.clone(), resolver.clone(), true).await;
        assert!(error.err().unwrap().to_string().contains("late.test"));

        let pool = Pool::connect(upstreams, resolver.clone(), false)
            .await
            .unwrap();
        assert!(pool.unresolved());
        assert_eq!(pool.status().len(), 1, "the literal upstream still serves");
        resolver.set("late.test", &["10.0.0.7"]);
        pool.refresh_dns().await;
        assert!(!pool.unresolved());
        assert_eq!(pool.status().len(), 2);
    }

    #[tokio::test]
    async fn admission_limit_is_enforced_and_released() {
        let pool = Arc::new(pool(&["http://10.0.0.1:1"], Balancing::RoundRobin).await);
        let first = pool.admit(Some(2)).unwrap();
        let _second = pool.admit(Some(2)).unwrap();
        assert!(pool.admit(Some(2)).is_none());
        assert_eq!(pool.active(), 2);
        drop(first);
        assert!(pool.admit(Some(2)).is_some());
        assert!(pool.admit(None).is_some(), "no limit admits freely");
    }

    #[test]
    fn active_checks_flip_state_only_after_the_threshold() {
        let check = ActiveCheck::default();
        let backend = Backend::new(
            Arc::new(Origin::parse("http://10.0.0.1:80").unwrap()),
            "10.0.0.1:80".parse().unwrap(),
        );
        assert_eq!(backend.check_result(false, &check), None);
        assert_eq!(
            backend.check_result(true, &check),
            None,
            "a success resets the streak"
        );
        assert_eq!(backend.check_result(false, &check), None);
        assert_eq!(backend.check_result(false, &check), Some(false));
        assert!(!backend.available(0));
        assert_eq!(backend.check_result(true, &check), None);
        assert_eq!(backend.check_result(true, &check), Some(true));
        assert!(backend.available(0));
    }
}
