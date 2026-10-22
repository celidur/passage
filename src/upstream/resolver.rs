//! Name resolution behind a trait, so refresh behaviour can be tested without a real DNS.

use async_trait::async_trait;
use std::{io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::task::JoinSet;

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

#[async_trait]
pub trait Resolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>>;
}

/// The operating system resolver; it honours `/etc/hosts` and the configured name servers.
pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
        match tokio::time::timeout(LOOKUP_TIMEOUT, tokio::net::lookup_host((host, port))).await {
            Ok(addresses) => Ok(addresses?.collect()),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "DNS lookup timed out",
            )),
        }
    }
}

/// Resolves every `(host, port)` concurrently; results keep the order of `names`.
pub async fn resolve_all(
    resolver: &Arc<dyn Resolver>,
    names: Vec<(String, u16)>,
) -> Vec<io::Result<Vec<SocketAddr>>> {
    let mut lookups = JoinSet::new();
    for (index, (host, port)) in names.iter().cloned().enumerate() {
        let resolver = resolver.clone();
        lookups.spawn(async move { (index, resolver.resolve(&host, port).await) });
    }
    let mut results: Vec<_> = names
        .iter()
        .map(|_| Err(io::Error::other("lookup task failed")))
        .collect();
    while let Some(joined) = lookups.join_next().await {
        if let Ok((index, result)) = joined {
            results[index] = result;
        }
    }
    results
}
