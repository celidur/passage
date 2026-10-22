//! Issues and renews certificates in the background.

use super::{Certificate, Certificates, material::load, now};
use async_trait::async_trait;
use pingora::{server::ShutdownWatch, services::background::BackgroundService};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use tokio::time::{Instant, MissedTickBehavior, interval_at};

const CHECK_EVERY: Duration = Duration::from_secs(60);
const MAX_ERROR_CHARS: usize = 500;

/// Marks a certificate as being issued for as long as it lives.
struct Busy<'a>(&'a Mutex<HashSet<String>>, String);

impl<'a> Busy<'a> {
    fn new(set: &'a Mutex<HashSet<String>>, id: &str) -> Self {
        set.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_string());
        Self(set, id.to_string())
    }
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.1);
    }
}

impl Certificates {
    /// Starts an issuance for every certificate that is due.
    pub(super) async fn process_due(&self) {
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
            if item.certificate.is_due(now, renew_before) {
                self.issue_one(item.certificate).await;
            }
        }
    }

    async fn issue_one(&self, certificate: Certificate) {
        let Some(issuer) = &self.issuer else {
            return;
        };
        let _busy = Busy::new(&self.issuing, &certificate.id);
        tracing::info!(
            certificate = certificate.id,
            domains = ?certificate.domains,
            "requesting a certificate"
        );
        let outcome = async {
            let issued = issuer
                .issue(&certificate.domains, certificate.challenge)
                .await?;
            let loaded = load(&issued.chain_pem, &issued.key_pem)?;
            anyhow::Ok((issued, loaded))
        }
        .await;

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

pub struct CertificateManager(pub Arc<Certificates>);

#[async_trait]
impl BackgroundService for CertificateManager {
    async fn start(&self, mut shutdown: ShutdownWatch) {
        let mut ticker = interval_at(Instant::now() + CHECK_EVERY, CHECK_EVERY);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.changed() => return,
                _ = self.0.process_due() => {}
            }
            tokio::select! {
                _ = shutdown.changed() => return,
                _ = ticker.tick() => {}
                _ = self.0.wake.notified() => {}
            }
        }
    }
}
