//! Active health probes.

use super::pool::Backend;
use crate::routes::policy::{ActiveCheck, CheckKind};
use std::time::Duration;
use tokio::net::TcpStream;

/// Probes one backend. Any failure, including a timeout, counts as unhealthy.
pub async fn probe(backend: &Backend, check: &ActiveCheck) -> bool {
    let timeout = Duration::from_secs(check.timeout_secs);
    match check.kind {
        CheckKind::Tcp => matches!(
            tokio::time::timeout(timeout, TcpStream::connect(backend.address)).await,
            Ok(Ok(_))
        ),
        CheckKind::Http => http(backend, check, timeout).await.unwrap_or(false),
    }
}

/// Sends the request to the backend's own address, with the same `Host` and certificate
/// name a proxied request would use.
async fn http(backend: &Backend, check: &ActiveCheck, timeout: Duration) -> reqwest::Result<bool> {
    let origin = &backend.origin;
    let scheme = if origin.tls { "https" } else { "http" };
    let mut client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .pool_max_idle_per_host(0)
        .user_agent("passage-health-check");
    if origin.literal.is_none() {
        client = client.resolve(&origin.sni, backend.address);
    }
    let response = client
        .build()?
        .get(format!("{scheme}://{}{}", origin.host_header, check.path))
        .send()
        .await?;
    let status = response.status();
    Ok(status.is_success() || status.is_redirection())
}
