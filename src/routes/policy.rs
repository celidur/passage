//! Per-route proxy policies: load balancing, timeouts, limits and upstream health.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const MAX_UPSTREAMS: usize = 16;
pub const MAX_RETRIES: u32 = 5;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Balancing {
    #[default]
    RoundRobin,
    LeastConnections,
}

/// How Passage talks to the upstream. `Http2` is required for gRPC upstreams.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamProtocol {
    /// HTTP/1.1 only.
    #[default]
    Http1,
    /// Negotiated with ALPN on HTTPS upstreams; HTTP/1.1 on plain HTTP.
    Auto,
    /// HTTP/2 only: prior knowledge (h2c) on plain HTTP, ALPN on HTTPS.
    Http2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Timeouts {
    /// TCP connect and TLS handshake budget.
    pub connect_secs: u64,
    /// Maximum silence while waiting for the upstream response or its next body chunk.
    pub read_secs: u64,
    /// Maximum time a single write to the upstream may block.
    pub write_secs: u64,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect_secs: 5,
            read_secs: 60,
            write_secs: 60,
        }
    }
}

impl Timeouts {
    pub fn connect(&self) -> Duration {
        Duration::from_secs(self.connect_secs)
    }

    pub fn read(&self) -> Duration {
        Duration::from_secs(self.read_secs)
    }

    pub fn write(&self) -> Duration {
        Duration::from_secs(self.write_secs)
    }

    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("connect_secs", self.connect_secs),
            ("read_secs", self.read_secs),
            ("write_secs", self.write_secs),
        ] {
            ensure!(
                (1..=3600).contains(&value),
                "timeouts.{name} must be between 1 and 3600"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Concurrent requests (including WebSocket and streaming calls) admitted for the route.
    pub max_connections: Option<u32>,
    /// Largest accepted request body; larger requests receive 413.
    pub max_request_body_bytes: Option<u64>,
}

impl Limits {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.max_connections
                .is_none_or(|n| (1..=1_000_000).contains(&n)),
            "limits.max_connections must be between 1 and 1000000"
        );
        ensure!(
            self.max_request_body_bytes.is_none_or(|n| n >= 1),
            "limits.max_request_body_bytes must be at least 1"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// GET on `path`; any 2xx or 3xx status is healthy. HTTP/1.1 only.
    #[default]
    Http,
    /// A TCP connection is enough. Use it for HTTP/2-only upstreams such as gRPC.
    Tcp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ActiveCheck {
    pub kind: CheckKind,
    pub path: String,
    pub interval_secs: u64,
    pub timeout_secs: u64,
    /// Consecutive successes before a failed upstream receives traffic again.
    pub healthy_threshold: u32,
    /// Consecutive failures before an upstream stops receiving traffic.
    pub unhealthy_threshold: u32,
}

impl Default for ActiveCheck {
    fn default() -> Self {
        Self {
            kind: CheckKind::Http,
            path: "/".into(),
            interval_secs: 10,
            timeout_secs: 3,
            healthy_threshold: 2,
            unhealthy_threshold: 2,
        }
    }
}

impl ActiveCheck {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.path.starts_with('/')
                && self.path.len() <= 2048
                && !self.path.contains('#')
                && !self.path.chars().any(|c| c.is_control() || c == ' '),
            "health.check.path must start with '/' and contain no spaces or fragment"
        );
        ensure!(
            (1..=3600).contains(&self.interval_secs),
            "health.check.interval_secs must be between 1 and 3600"
        );
        ensure!(
            (1..=60).contains(&self.timeout_secs) && self.timeout_secs <= self.interval_secs,
            "health.check.timeout_secs must be between 1 and 60 and not exceed the interval"
        );
        ensure!(
            (1..=10).contains(&self.healthy_threshold)
                && (1..=10).contains(&self.unhealthy_threshold),
            "health.check thresholds must be between 1 and 10"
        );
        Ok(())
    }
}

/// Passive health always applies; the active check is opt-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Health {
    /// Consecutive connection failures before an upstream is set aside.
    pub max_failures: u32,
    /// How long a failed upstream is skipped before it is tried again.
    pub cooldown_secs: u64,
    pub check: Option<ActiveCheck>,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            max_failures: 3,
            cooldown_secs: 10,
            check: None,
        }
    }
}

impl Health {
    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=100).contains(&self.max_failures),
            "health.max_failures must be between 1 and 100"
        );
        ensure!(
            (1..=3600).contains(&self.cooldown_secs),
            "health.cooldown_secs must be between 1 and 3600"
        );
        self.check.iter().try_for_each(ActiveCheck::validate)
    }
}

/// Everything about a route that is not identity, matching or authentication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    pub balancing: Balancing,
    pub protocol: UpstreamProtocol,
    /// Extra attempts on other upstreams after a connection failure.
    pub retries: u32,
    pub timeouts: Timeouts,
    pub limits: Limits,
    pub health: Health,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            balancing: Balancing::default(),
            protocol: UpstreamProtocol::default(),
            retries: 2,
            timeouts: Timeouts::default(),
            limits: Limits::default(),
            health: Health::default(),
        }
    }
}

impl Policy {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.retries <= MAX_RETRIES,
            "retries must be at most {MAX_RETRIES}"
        );
        self.timeouts.validate()?;
        self.limits.validate()?;
        self.health.validate()
    }
}
