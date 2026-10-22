//! Obtaining a certificate from an ACME certificate authority.

use super::{challenges::ChallengeStore, material, model::Challenge as Kind};
use crate::{config::Acme, store::Store};
use anyhow::{Context, Result, anyhow, bail, ensure};
use instant_acme::{
    Account, AuthorizationStatus, ChallengeType, Identifier, NewAccount, NewOrder, Order,
    OrderStatus, RetryPolicy,
};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{process::Command, sync::Mutex};

const HOOK_TIMEOUT: Duration = Duration::from_secs(60);

/// What the issuer needs from `[acme]`.
pub struct Settings {
    pub email: String,
    pub directory_url: String,
    pub ca_certificate: Option<PathBuf>,
    pub dns_hook: Option<PathBuf>,
    pub dns_propagation: Duration,
    pub renew_before: Duration,
}

impl From<&Acme> for Settings {
    fn from(acme: &Acme) -> Self {
        Self {
            email: acme.email.clone(),
            directory_url: acme.directory_url.clone(),
            ca_certificate: acme.ca_certificate.clone(),
            dns_hook: acme.dns_hook.clone(),
            dns_propagation: Duration::from_secs(acme.dns_propagation_secs),
            renew_before: acme.renew_before(),
        }
    }
}

pub struct Issued {
    pub chain_pem: String,
    pub key_pem: String,
}

pub struct Issuer {
    settings: Settings,
    store: Arc<Store>,
    proofs: Arc<ChallengeStore>,
    account: Mutex<Option<Account>>,
}

/// Proofs published for one order, so they can be withdrawn whatever the outcome.
#[derive(Default)]
struct Published {
    tokens: Vec<String>,
    /// DNS record name and value.
    records: Vec<(String, String)>,
}

fn challenge_type(kind: Kind) -> ChallengeType {
    match kind {
        Kind::Http01 => ChallengeType::Http01,
        Kind::Dns01 => ChallengeType::Dns01,
    }
}

impl Issuer {
    pub fn new(settings: Settings, store: Arc<Store>, proofs: Arc<ChallengeStore>) -> Self {
        Self {
            settings,
            store,
            proofs,
            account: Mutex::new(None),
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn supports_dns(&self) -> bool {
        self.settings.dns_hook.is_some()
    }

    /// Runs a complete order for `domains` and returns the chain with its new private key.
    pub async fn issue(&self, domains: &[String], kind: Kind) -> Result<Issued> {
        let account = self.account().await?;
        let identifiers: Vec<_> = domains.iter().cloned().map(Identifier::Dns).collect();
        let mut order = account
            .new_order(&NewOrder::new(&identifiers))
            .await
            .context("the certificate authority refused the order")?;
        let mut published = Published::default();
        let result = self
            .complete(&mut order, domains, kind, &mut published)
            .await;
        self.withdraw(published).await;
        result
    }

    async fn complete(
        &self,
        order: &mut Order,
        domains: &[String],
        kind: Kind,
        published: &mut Published,
    ) -> Result<Issued> {
        let mut pending = 0;
        let mut authorizations = order.authorizations();
        while let Some(authorization) = authorizations.next().await {
            let mut authorization = authorization.context("cannot read an authorization")?;
            let name = authorization.identifier().to_string();
            match authorization.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                status => bail!("the authorization for {name} is {status:?}"),
            }
            let challenge = authorization
                .challenge(challenge_type(kind))
                .with_context(|| {
                    format!("the certificate authority offers no {kind:?} challenge for {name}")
                })?;
            let proof = challenge.key_authorization();
            match kind {
                Kind::Http01 => {
                    self.proofs
                        .insert(challenge.token.clone(), proof.as_str().to_owned());
                    published.tokens.push(challenge.token.clone());
                }
                Kind::Dns01 => {
                    let Identifier::Dns(host) = challenge.identifier().identifier else {
                        bail!("{name} is not a DNS name");
                    };
                    let (record, value) = (format!("_acme-challenge.{host}"), proof.dns_value());
                    self.run_hook("present", &record, &value).await?;
                    published.records.push((record, value));
                }
            }
            pending += 1;
        }
        if kind == Kind::Dns01 && pending > 0 {
            tokio::time::sleep(self.settings.dns_propagation).await;
        }

        let mut authorizations = order.authorizations();
        while let Some(authorization) = authorizations.next().await {
            let mut authorization = authorization.context("cannot read an authorization")?;
            if !matches!(authorization.status, AuthorizationStatus::Pending) {
                continue;
            }
            authorization
                .challenge(challenge_type(kind))
                .context("the challenge disappeared")?
                .set_ready()
                .await
                .context("cannot start the validation")?;
        }

        let policy = RetryPolicy::new()
            .initial_delay(Duration::from_millis(500))
            .timeout(Duration::from_secs(120));
        let ready = order.poll_ready(&policy).await;
        if !matches!(ready, Ok(OrderStatus::Ready)) {
            let reasons = rejection(order, kind).await;
            let cause = match ready {
                Ok(status) => format!("the order is {status:?}"),
                Err(error) => error.to_string(),
            };
            bail!(
                "{}",
                if reasons.is_empty() {
                    cause
                } else {
                    format!("{cause}: {reasons}")
                }
            );
        }

        let (key_pem, csr) = material::new_key_and_csr(domains)?;
        order
            .finalize_csr(&csr)
            .await
            .context("cannot finalize the order")?;
        let chain_pem = order
            .poll_certificate(&policy)
            .await
            .context("the certificate was not issued")?;
        Ok(Issued { chain_pem, key_pem })
    }

    /// The registered account, created on first use. Credentials are kept per directory, so
    /// switching from a staging to a production CA registers a new account.
    async fn account(&self) -> Result<Account> {
        let mut cached = self.account.lock().await;
        if let Some(account) = &*cached {
            return Ok(account.clone());
        }
        let builder = || {
            match &self.settings.ca_certificate {
                Some(root) => Account::builder_with_root(root),
                None => Account::builder(),
            }
            .context("cannot prepare the ACME client")
        };
        let key = format!("acme_account:{}", self.settings.directory_url);
        let account = match self.store.setting(&key)? {
            Some(saved) => builder()?
                .from_credentials(serde_json::from_str(&saved)?)
                .await
                .context("cannot restore the ACME account")?,
            None => {
                let contact = format!("mailto:{}", self.settings.email);
                let (account, credentials) = builder()?
                    .create(
                        &NewAccount {
                            contact: &[contact.as_str()],
                            terms_of_service_agreed: true,
                            only_return_existing: false,
                        },
                        self.settings.directory_url.clone(),
                        None,
                    )
                    .await
                    .context("cannot register an ACME account")?;
                self.store
                    .set_setting(&key, &serde_json::to_string(&credentials)?)?;
                tracing::info!(
                    directory = self.settings.directory_url,
                    "ACME account registered"
                );
                account
            }
        };
        *cached = Some(account.clone());
        Ok(account)
    }

    async fn withdraw(&self, published: Published) {
        for token in &published.tokens {
            self.proofs.remove(token);
        }
        for (record, value) in &published.records {
            if let Err(error) = self.run_hook("cleanup", record, value).await {
                tracing::warn!(record, %error, "cannot remove a DNS challenge record");
            }
        }
    }

    async fn run_hook(&self, action: &str, record: &str, value: &str) -> Result<()> {
        let hook = self
            .settings
            .dns_hook
            .as_ref()
            .ok_or_else(|| anyhow!("DNS-01 needs acme.dns_hook in the configuration"))?;
        let output = tokio::time::timeout(
            HOOK_TIMEOUT,
            Command::new(hook)
                .args([action, record, value])
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .with_context(|| format!("the DNS hook timed out on {action}"))?
        .context("cannot run the DNS hook")?;
        ensure!(
            output.status.success(),
            "the DNS hook failed on {action} ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(300)
                .collect::<String>()
        );
        Ok(())
    }
}

/// Why the CA rejected the challenges of an order, as one line.
async fn rejection(order: &mut Order, kind: Kind) -> String {
    let wanted = challenge_type(kind);
    let mut reasons = Vec::new();
    let mut authorizations = order.authorizations();
    while let Some(Ok(mut authorization)) = authorizations.next().await {
        if authorization.refresh().await.is_err() {
            continue;
        }
        let name = authorization.identifier().to_string();
        for challenge in authorization
            .challenges
            .iter()
            .filter(|c| c.r#type == wanted)
        {
            if let Some(problem) = &challenge.error {
                let detail = problem.detail.as_deref().unwrap_or("validation failed");
                reasons.push(format!("{name}: {detail}"));
            }
        }
    }
    reasons.join("; ")
}
