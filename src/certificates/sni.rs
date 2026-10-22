//! Choosing a certificate per TLS server name, swapped atomically so renewals never drop traffic.

use super::{material::CertifiedKey, model::covers};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use pingora::{
    listeners::TlsAccept,
    tls::{
        ext::{ssl_add_chain_cert, ssl_use_certificate, ssl_use_private_key},
        ssl::{NameType, SslRef},
    },
};
use std::{collections::HashMap, sync::Arc};

/// Everything a handshake can pick from. Immutable; a new one replaces it whole.
#[derive(Default)]
pub struct Keys {
    exact: HashMap<String, Arc<CertifiedKey>>,
    /// Keyed by the parent domain: `*.example.com` is stored under `example.com`.
    wildcard: HashMap<String, Arc<CertifiedKey>>,
    fallback: Option<Arc<CertifiedKey>>,
}

/// A certificate, the names it serves and when it expires.
pub struct Entry {
    pub domains: Vec<String>,
    pub key: Arc<CertifiedKey>,
    pub not_after: i64,
}

impl Keys {
    /// When several certificates serve a name, the one expiring last wins.
    pub fn build(entries: Vec<Entry>, fallback: Option<Arc<CertifiedKey>>) -> Self {
        let mut keys = Self {
            fallback,
            ..Self::default()
        };
        let mut expiry: HashMap<(bool, String), i64> = HashMap::new();
        for entry in entries {
            for domain in &entry.domains {
                let (wildcard, name) = match domain.strip_prefix("*.") {
                    Some(parent) => (true, parent.to_string()),
                    None => (false, domain.clone()),
                };
                let slot = (wildcard, name.clone());
                if expiry
                    .get(&slot)
                    .is_some_and(|&seen| seen >= entry.not_after)
                {
                    continue;
                }
                expiry.insert(slot, entry.not_after);
                let map = if wildcard {
                    &mut keys.wildcard
                } else {
                    &mut keys.exact
                };
                map.insert(name, entry.key.clone());
            }
        }
        keys
    }

    /// An exact match beats a wildcard, and both beat the fallback.
    pub fn select(&self, server_name: &str) -> Option<&Arc<CertifiedKey>> {
        let name = server_name.trim_end_matches('.').to_ascii_lowercase();
        self.exact
            .get(&name)
            .or_else(|| {
                let (label, parent) = name.split_once('.')?;
                if label.is_empty() {
                    return None;
                }
                self.wildcard.get(parent)
            })
            .or(self.fallback.as_ref())
    }

    pub fn has_fallback(&self) -> bool {
        self.fallback.is_some()
    }

    /// Whether a managed certificate (not the fallback) serves `host`.
    pub fn serves(&self, host: &str) -> bool {
        self.exact.contains_key(host)
            || self
                .wildcard
                .keys()
                .any(|parent| covers(&format!("*.{parent}"), host))
    }
}

/// The live certificate set, shared by the TLS listener and the certificate manager.
#[derive(Clone, Default)]
pub struct SniKeys(Arc<ArcSwap<Keys>>);

impl SniKeys {
    pub fn load(&self) -> arc_swap::Guard<Arc<Keys>> {
        self.0.load()
    }

    pub fn replace(&self, keys: Keys) {
        self.0.store(Arc::new(keys));
    }
}

/// Plugs the live certificate set into Pingora's TLS handshake.
pub struct SniResolver(pub SniKeys);

#[async_trait]
impl TlsAccept for SniResolver {
    async fn certificate_callback(&self, ssl: &mut SslRef) {
        let name = ssl
            .servername(NameType::HOST_NAME)
            .unwrap_or_default()
            .to_string();
        let keys = self.0.load();
        let Some(selected) = keys.select(&name) else {
            return;
        };
        let installed = ssl_use_certificate(ssl, &selected.leaf)
            .and_then(|()| ssl_use_private_key(ssl, &selected.key))
            .and_then(|()| {
                selected
                    .chain
                    .iter()
                    .try_for_each(|cert| ssl_add_chain_cert(ssl, cert))
            });
        if let Err(error) = installed {
            tracing::warn!(server_name = name, %error, "cannot install the certificate for a handshake");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certificates::material::{load, testing::self_signed};

    fn key(name: &str) -> Arc<CertifiedKey> {
        let (chain, key) = self_signed(&[name], 1);
        Arc::new(load(&chain, &key).unwrap().certified)
    }

    fn entry(domains: &[&str], key: &Arc<CertifiedKey>, not_after: i64) -> Entry {
        Entry {
            domains: domains.iter().map(|d| d.to_string()).collect(),
            key: key.clone(),
            not_after,
        }
    }

    #[test]
    fn exact_beats_wildcard_beats_fallback() {
        let (exact, wildcard, fallback) = (key("a"), key("b"), key("c"));
        let keys = Keys::build(
            vec![
                entry(&["app.example.com"], &exact, 10),
                entry(&["*.example.com"], &wildcard, 10),
            ],
            Some(fallback.clone()),
        );
        let is = |name: &str, expected: &Arc<CertifiedKey>| {
            Arc::ptr_eq(keys.select(name).unwrap(), expected)
        };
        assert!(is("app.example.com", &exact));
        assert!(
            is("APP.Example.com.", &exact),
            "case and trailing dot are ignored"
        );
        assert!(is("www.example.com", &wildcard));
        assert!(
            is("a.b.example.com", &fallback),
            "a wildcard covers one label only"
        );
        assert!(is("example.com", &fallback));
        assert!(is("", &fallback), "no SNI uses the fallback");
        assert!(keys.serves("www.example.com") && !keys.serves("example.org"));
    }

    #[test]
    fn without_a_fallback_unknown_names_get_nothing() {
        let keys = Keys::build(vec![entry(&["app.example.com"], &key("a"), 10)], None);
        assert!(keys.select("other.example.com").is_none());
        assert!(!keys.has_fallback());
    }

    #[test]
    fn the_longest_lived_certificate_wins_a_shared_name() {
        let (old, new) = (key("old"), key("new"));
        for entries in [
            vec![
                entry(&["app.example.com"], &old, 10),
                entry(&["app.example.com"], &new, 20),
            ],
            vec![
                entry(&["app.example.com"], &new, 20),
                entry(&["app.example.com"], &old, 10),
            ],
        ] {
            let keys = Keys::build(entries, None);
            assert!(Arc::ptr_eq(keys.select("app.example.com").unwrap(), &new));
        }
    }

    #[test]
    fn replacing_the_set_is_seen_by_the_next_handshake() {
        let shared = SniKeys::default();
        assert!(shared.load().select("app.example.com").is_none());
        shared.replace(Keys::build(
            vec![entry(&["app.example.com"], &key("a"), 1)],
            None,
        ));
        assert!(shared.load().select("app.example.com").is_some());
    }
}
