//! Managed TLS certificates: selection per server name, ACME issuance and renewal.

mod acme;
mod challenges;
mod manager;
mod material;
mod model;
mod sni;

pub use challenges::ChallengeStore;
pub use manager::CertificateManager;
pub use model::{Certificate, CertificateView, Challenge, NewCertificate, Status, covers};
pub use sni::SniResolver;

use crate::{config::Config, state::UpdateError, store::Store};
use acme::{Issuer, Settings};
use anyhow::{Context, Result, ensure};
use material::{CertifiedKey, load};
use serde::Serialize;
use sni::{Entry, Keys, SniKeys};
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// What this server can do about HTTPS, as configured.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HttpsFeatures {
    /// An HTTPS listener exists.
    pub enabled: bool,
    /// Certificates can be requested from an ACME authority.
    pub acme: bool,
    /// Wildcard certificates (DNS-01) are possible.
    pub dns_01: bool,
    /// Port used in HTTP to HTTPS redirects.
    #[serde(skip)]
    pub public_port: u16,
}

pub struct Certificates {
    store: Arc<Store>,
    keys: SniKeys,
    /// Used for names no managed certificate serves.
    fallback: Option<Arc<CertifiedKey>>,
    pub proofs: Arc<ChallengeStore>,
    issuer: Option<Issuer>,
    features: HttpsFeatures,
    wake: Notify,
    issuing: Mutex<HashSet<String>>,
    /// Serializes changes to certificate records.
    edit: tokio::sync::Mutex<()>,
}

impl Certificates {
    pub fn new(store: Arc<Store>, config: &Config) -> Result<Self> {
        let fallback = config
            .https
            .as_ref()
            .and_then(|https| {
                https
                    .certificate
                    .as_deref()
                    .zip(https.private_key.as_deref())
            })
            .map(|(certificate, key)| load_static(certificate, key))
            .transpose()?;
        let proofs = Arc::new(ChallengeStore::default());
        let issuer = config
            .acme
            .as_ref()
            .map(|acme| Issuer::new(Settings::from(acme), store.clone(), proofs.clone()));
        let features = HttpsFeatures {
            enabled: config.https.is_some(),
            acme: issuer.is_some(),
            dns_01: issuer.as_ref().is_some_and(Issuer::supports_dns),
            public_port: config
                .https
                .as_ref()
                .map_or(443, |https| https.listen.port()),
        };
        let certificates = Self {
            store,
            keys: SniKeys::default(),
            fallback,
            proofs,
            issuer,
            features,
            wake: Notify::new(),
            issuing: Mutex::new(HashSet::new()),
            edit: tokio::sync::Mutex::new(()),
        };
        certificates.reload()?;
        Ok(certificates)
    }

    pub fn features(&self) -> &HttpsFeatures {
        &self.features
    }

    /// The handshake hook for the HTTPS listener.
    pub fn resolver(&self) -> SniResolver {
        SniResolver(self.keys.clone())
    }

    /// Rebuilds the live certificate set from storage. In-flight connections keep their
    /// certificate; the next handshake sees the new one.
    pub fn reload(&self) -> Result<()> {
        let mut entries = Vec::new();
        for stored in self.store.certificates()? {
            let Some((chain, key)) = &stored.material else {
                continue;
            };
            match load(chain, key) {
                Ok(loaded) => entries.push(Entry {
                    domains: stored.certificate.domains.clone(),
                    key: Arc::new(loaded.certified),
                    not_after: loaded.not_after,
                }),
                Err(error) => tracing::error!(
                    certificate = stored.certificate.id,
                    %error,
                    "a stored certificate is unusable"
                ),
            }
        }
        self.keys
            .replace(Keys::build(entries, self.fallback.clone()));
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<CertificateView>> {
        Ok(self
            .store
            .certificates()?
            .into_iter()
            .map(|stored| self.view(stored.certificate))
            .collect())
    }

    fn view(&self, certificate: Certificate) -> CertificateView {
        let issuing = self
            .issuing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&certificate.id);
        CertificateView {
            status: certificate.status(now(), issuing),
            certificate,
        }
    }

    fn issuer(&self) -> Result<&Issuer> {
        self.issuer.as_ref().context(
            "automatic certificates are not configured: add an [acme] section to passage.toml",
        )
    }

    fn new_record(domains: Vec<String>, challenge: Challenge) -> Certificate {
        Certificate {
            id: uuid::Uuid::new_v4().to_string(),
            domains,
            challenge,
            created_at: now(),
            not_before: None,
            not_after: None,
            last_error: None,
            failures: 0,
            next_attempt: None,
            renew_requested: false,
        }
    }

    /// Records a new certificate and wakes the manager to obtain it.
    pub async fn create(
        &self,
        mut request: NewCertificate,
    ) -> Result<CertificateView, UpdateError> {
        request.validate().map_err(UpdateError::Invalid)?;
        let issuer = self.issuer().map_err(UpdateError::Invalid)?;
        if request.challenge == Challenge::Dns01 && !issuer.supports_dns() {
            return Err(UpdateError::Invalid(anyhow::anyhow!(
                "dns-01 needs acme.dns_hook in passage.toml"
            )));
        }
        let _edit = self.edit.lock().await;
        let existing = self.store.certificates().map_err(UpdateError::Storage)?;
        let wanted: HashSet<_> = request.domains.iter().collect();
        if existing
            .iter()
            .any(|s| s.certificate.domains.iter().collect::<HashSet<_>>() == wanted)
        {
            return Err(UpdateError::Invalid(anyhow::anyhow!(
                "a certificate for these domains already exists"
            )));
        }
        let certificate = Self::new_record(request.domains, request.challenge);
        self.store
            .save_certificate(&certificate)
            .map_err(UpdateError::Storage)?;
        self.wake.notify_one();
        Ok(self.view(certificate))
    }

    /// Makes sure HTTPS can be served for `host`: a managed certificate already covers it, or one
    /// is requested over HTTP-01, or the fallback certificate stands in.
    pub async fn ensure_for(&self, host: &str) -> Result<()> {
        ensure!(
            self.features.enabled,
            "HTTPS is not enabled on this server: add a [tls] section to passage.toml"
        );
        let _edit = self.edit.lock().await;
        let existing = self.store.certificates()?;
        if existing.iter().any(|s| s.certificate.covers_host(host)) {
            return Ok(());
        }
        if self.issuer.is_none() {
            ensure!(
                self.fallback.is_some(),
                "no certificate covers {host} and automatic certificates are not configured"
            );
            return Ok(());
        }
        let mut request = NewCertificate {
            domains: vec![host.to_string()],
            challenge: Challenge::Http01,
        };
        request
            .validate()
            .with_context(|| format!("cannot request a certificate for {host}"))?;
        self.store
            .save_certificate(&Self::new_record(request.domains, request.challenge))?;
        self.wake.notify_one();
        Ok(())
    }

    /// Asks for a new certificate now, whatever the current one's remaining life.
    pub async fn renew(&self, id: &str) -> Result<CertificateView, UpdateError> {
        self.issuer().map_err(UpdateError::Invalid)?;
        let _edit = self.edit.lock().await;
        let mut certificate = self
            .store
            .certificate(id)
            .map_err(UpdateError::Storage)?
            .ok_or(UpdateError::NotFound)?;
        certificate.renew_requested = true;
        certificate.next_attempt = None;
        self.store
            .save_certificate(&certificate)
            .map_err(UpdateError::Storage)?;
        self.wake.notify_one();
        Ok(self.view(certificate))
    }

    pub async fn delete(&self, id: &str) -> Result<(), UpdateError> {
        let _edit = self.edit.lock().await;
        if !self
            .store
            .delete_certificate(id)
            .map_err(UpdateError::Storage)?
        {
            return Err(UpdateError::NotFound);
        }
        self.reload().map_err(UpdateError::Storage)
    }
}

fn load_static(certificate: &Path, key: &Path) -> Result<Arc<CertifiedKey>> {
    let chain = std::fs::read_to_string(certificate)
        .with_context(|| format!("cannot read {}", certificate.display()))?;
    let key =
        std::fs::read_to_string(key).with_context(|| format!("cannot read {}", key.display()))?;
    Ok(Arc::new(load(&chain, &key)?.certified))
}
