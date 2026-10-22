//! Issues and renews certificates in the background.

use super::{Certificate, Certificates, material::load, now};
use async_trait::async_trait;
use pingora::{server::ShutdownWatch, services::background::BackgroundService};
use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, PoisonError},
    time::Duration,
};
use tokio::{
    net::TcpStream,
    task::JoinSet,
    time::{Instant, MissedTickBehavior, interval_at},
};

const CHECK_EVERY: Duration = Duration::from_secs(60);
const MAX_ERROR_CHARS: usize = 500;

/// Marks a certificate as being issued for as long as it lives.
struct Busy(Arc<Certificates>, String);

impl Busy {
    /// `None` when an issuance for this certificate is already running.
    fn claim(certificates: &Arc<Certificates>, id: &str) -> Option<Self> {
        let claimed = certificates
            .issuing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_string());
        claimed.then(|| Self(certificates.clone(), id.to_string()))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0
            .issuing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.1);
    }
}

impl Certificates {
    /// Starts an issuance for every certificate that is due and not already being issued.
    fn start_due(self: &Arc<Self>, tasks: &mut JoinSet<()>, shutdown: &ShutdownWatch) {
        let Some(issuer) = &self.issuer else {
            return;
        };
        let renew_before = issuer.settings().renew_before.as_secs() as i64;
        let stored = match self.store.certificates() {
            Ok(stored) => stored,
            Err(error) => {
                tracing::error!(%error, "cannot read the certificates");
                return;
            }
        };
        let now = now();
        for item in stored {
            let certificate = item.certificate;
            if !certificate.is_due(now, renew_before) {
                continue;
            }
            let Some(busy) = Busy::claim(self, &certificate.id) else {
                continue;
            };
            let shutdown = shutdown.clone();
            tasks.spawn(async move { busy.0.issue_one(certificate, shutdown).await });
        }
    }

    async fn issue_one(&self, certificate: Certificate, shutdown: ShutdownWatch) {
        let Some(issuer) = &self.issuer else {
            return;
        };
        tracing::info!(
            certificate = certificate.id,
            domains = ?certificate.domains,
            "requesting a certificate"
        );
        let outcome = async {
            let issued = issuer
                .issue(
                    &certificate.domains,
                    certificate.challenge,
                    shutdown.clone(),
                )
                .await?;
            let loaded = load(&issued.chain_pem, &issued.key_pem)?;
            anyhow::Ok((issued, loaded))
        }
        .await;
        if outcome.is_err() && *shutdown.borrow() {
            // Not a failure of the certificate: retry right away on the next start.
            return;
        }

        let _edit = self.edit.lock().await;
        // The certificate may have been deleted while the CA was working.
        let Ok(Some(mut current)) = self.store.certificate(&certificate.id) else {
            return;
        };
        current.renew_requested = false;
        let saved = match outcome {
            Ok((issued, loaded)) => {
                current.not_before = Some(loaded.not_before);
                current.not_after = Some(loaded.not_after);
                current.last_error = None;
                current.failures = 0;
                current.next_attempt = None;
                self.store
                    .save_material(&current, &issued.chain_pem, &issued.key_pem)
                    .and_then(|()| self.reload())
                    .inspect(|()| {
                        tracing::info!(certificate = current.id, domains = ?current.domains, "certificate issued")
                    })
            }
            Err(error) => {
                let message: String = format!("{error:#}").chars().take(MAX_ERROR_CHARS).collect();
                current.failures += 1;
                current.next_attempt = Some(now() + Certificate::backoff(current.failures));
                tracing::warn!(
                    certificate = current.id,
                    domains = ?current.domains,
                    error = message,
                    "certificate request failed"
                );
                current.last_error = Some(message);
                self.store.save_certificate(&current)
            }
        };
        if let Err(error) = saved {
            tracing::error!(certificate = current.id, %error, "cannot save the certificate");
        }
    }
}

pub struct CertificateManager {
    pub certificates: Arc<Certificates>,
    /// The plain HTTP listener, which serves HTTP-01 proofs.
    pub http: SocketAddr,
}

#[async_trait]
impl BackgroundService for CertificateManager {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = wait_for_listener(self.http) => {}
        }
        let certificates = &self.certificates;
        let mut tasks = JoinSet::new();
        let mut ticker = interval_at(Instant::now() + CHECK_EVERY, CHECK_EVERY);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            if let Err(error) = certificates.request_missing().await {
                tracing::error!(
                    error = format!("{error:#}"),
                    "cannot order missing certificates"
                );
            }
            certificates.start_due(&mut tasks, &shutdown);
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = ticker.tick() => {}
                _ = certificates.wake.notified() => {}
                Some(_) = tasks.join_next() => {}
            }
        }
        // Interrupted orders withdraw their proofs before the process exits.
        while tasks.join_next().await.is_some() {}
    }
}

/// Waits a few seconds at most for the HTTP listener to accept connections.
async fn wait_for_listener(mut address: SocketAddr) {
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            Ipv4Addr::LOCALHOST.into()
        } else {
            Ipv6Addr::LOCALHOST.into()
        });
    }
    for _ in 0..50 {
        if TcpStream::connect(address).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tracing::warn!(%address, "the HTTP listener is not answering; HTTP-01 validation may fail");
}
