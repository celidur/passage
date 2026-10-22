//! Certificate records, domain validation and renewal scheduling.

use crate::routes::normalize_host;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_DOMAINS: usize = 10;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Challenge {
    /// Proof served over plain HTTP on port 80. Exact names only.
    #[default]
    #[serde(rename = "http-01")]
    Http01,
    /// Proof published as a DNS TXT record by the configured hook. Required for wildcards.
    #[serde(rename = "dns-01")]
    Dns01,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Waiting for the first attempt.
    Pending,
    Issuing,
    Valid,
    Expired,
    /// No certificate yet, and the last attempt failed.
    Failed,
}

/// A managed certificate. The key and chain are stored beside it, never in this record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Certificate {
    pub id: String,
    pub domains: Vec<String>,
    pub challenge: Challenge,
    pub created_at: i64,
    #[serde(default)]
    pub not_before: Option<i64>,
    #[serde(default)]
    pub not_after: Option<i64>,
    /// The latest failure; kept while a renewal fails and the current certificate still serves.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Consecutive failed attempts.
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub next_attempt: Option<i64>,
    #[serde(default)]
    pub renew_requested: bool,
}

#[derive(Debug, Serialize)]
pub struct CertificateView {
    #[serde(flatten)]
    pub certificate: Certificate,
    pub status: Status,
}

/// Fields accepted by `POST /api/certificates`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewCertificate {
    pub domains: Vec<String>,
    #[serde(default)]
    pub challenge: Challenge,
}

impl NewCertificate {
    /// Validates and normalizes the domain list in place.
    pub fn validate(&mut self) -> Result<()> {
        ensure!(
            (1..=MAX_DOMAINS).contains(&self.domains.len()),
            "provide between 1 and {MAX_DOMAINS} domains"
        );
        for domain in &mut self.domains {
            *domain = normalize_domain(domain)?;
        }
        let mut seen = HashSet::new();
        ensure!(
            self.domains.iter().all(|d| seen.insert(d.as_str())),
            "domains must be distinct"
        );
        ensure!(
            self.challenge == Challenge::Dns01 || self.domains.iter().all(|d| !d.starts_with("*.")),
            "wildcard domains need the dns-01 challenge"
        );
        Ok(())
    }
}

/// An exact public name, or `*.` followed by one.
pub fn normalize_domain(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    let (prefix, base) = match value.strip_prefix("*.") {
        Some(base) => ("*.", base),
        None => ("", value.as_str()),
    };
    let base = normalize_host(base)?;
    ensure!(
        base.contains('.') && !base.bytes().all(|b| b.is_ascii_digit() || b == b'.'),
        "{value:?} is not a public domain name"
    );
    Ok(format!("{prefix}{base}"))
}

/// Whether a certificate name covers a host; a wildcard covers exactly one extra label.
pub fn covers(domain: &str, host: &str) -> bool {
    match domain.strip_prefix("*.") {
        Some(base) => host
            .split_once('.')
            .is_some_and(|(label, rest)| !label.is_empty() && rest == base),
        None => domain == host,
    }
}

impl Certificate {
    pub fn covers_host(&self, host: &str) -> bool {
        self.domains.iter().any(|domain| covers(domain, host))
    }

    pub fn status(&self, now: i64, issuing: bool) -> Status {
        match self.not_after {
            _ if issuing => Status::Issuing,
            Some(expiry) if expiry <= now => Status::Expired,
            Some(_) => Status::Valid,
            None if self.last_error.is_some() => Status::Failed,
            None => Status::Pending,
        }
    }

    /// Whether an issuance attempt should start now. Renewal begins `renew_before` seconds
    /// ahead of expiry, or at two thirds of the lifetime for a short-lived certificate.
    pub fn is_due(&self, now: i64, renew_before: i64) -> bool {
        if self.next_attempt.is_some_and(|at| at > now) {
            return false;
        }
        match (self.not_before, self.not_after) {
            (Some(from), Some(to)) => {
                self.renew_requested || now >= to - renew_before.min((to - from) / 3)
            }
            _ => true,
        }
    }

    /// Seconds to wait after the `failures`-th consecutive failure: 5 minutes, doubling up to 5h20.
    pub fn backoff(failures: u32) -> i64 {
        300i64 << failures.saturating_sub(1).min(6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn certificate(not_before: Option<i64>, not_after: Option<i64>) -> Certificate {
        Certificate {
            id: "id".into(),
            domains: vec!["app.example.com".into()],
            challenge: Challenge::Http01,
            created_at: 0,
            not_before,
            not_after,
            last_error: None,
            failures: 0,
            next_attempt: None,
            renew_requested: false,
        }
    }

    #[test]
    fn domains_are_normalized_and_checked() {
        let mut request = NewCertificate {
            domains: vec![" App.Example.COM. ".into(), "*.Example.com".into()],
            challenge: Challenge::Dns01,
        };
        request.validate().unwrap();
        assert_eq!(request.domains, ["app.example.com", "*.example.com"]);

        for bad in [
            "localhost",
            "10.0.0.1",
            "*.com",
            "a_b.example.com",
            "*.*.example.com",
            "",
        ] {
            assert!(normalize_domain(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn wildcards_need_dns_01_and_lists_are_bounded() {
        let wildcard = |challenge| NewCertificate {
            domains: vec!["*.example.com".into()],
            challenge,
        };
        assert!(wildcard(Challenge::Http01).validate().is_err());
        assert!(wildcard(Challenge::Dns01).validate().is_ok());

        let mut empty = NewCertificate {
            domains: vec![],
            challenge: Challenge::Http01,
        };
        assert!(empty.validate().is_err());
        let mut duplicate = NewCertificate {
            domains: vec!["a.example.com".into(), "A.example.com".into()],
            challenge: Challenge::Http01,
        };
        assert!(duplicate.validate().is_err());
        let mut many = NewCertificate {
            domains: (0..11).map(|i| format!("h{i}.example.com")).collect(),
            challenge: Challenge::Http01,
        };
        assert!(many.validate().is_err());
    }

    #[test]
    fn a_wildcard_covers_exactly_one_label() {
        assert!(covers("*.example.com", "app.example.com"));
        assert!(!covers("*.example.com", "example.com"));
        assert!(!covers("*.example.com", "a.b.example.com"));
        assert!(!covers("*.example.com", "app.example.org"));
        assert!(covers("app.example.com", "app.example.com"));
        assert!(!covers("app.example.com", "www.example.com"));
    }

    #[test]
    fn renewal_starts_before_expiry_and_respects_backoff() {
        let day = 86_400;
        let renew_before = 30 * day;
        let issued = certificate(Some(0), Some(90 * day));
        assert!(!issued.is_due(59 * day, renew_before));
        assert!(issued.is_due(60 * day, renew_before));

        // A 6-day certificate renews at two thirds of its life, not constantly.
        let short = certificate(Some(0), Some(6 * day));
        assert!(!short.is_due(3 * day, renew_before));
        assert!(short.is_due(4 * day, renew_before));

        let mut waiting = certificate(None, None);
        assert!(waiting.is_due(0, renew_before));
        waiting.next_attempt = Some(100);
        assert!(!waiting.is_due(99, renew_before));
        assert!(waiting.is_due(100, renew_before));

        let mut forced = certificate(Some(0), Some(90 * day));
        forced.renew_requested = true;
        assert!(forced.is_due(day, renew_before));
    }

    #[test]
    fn failures_back_off_and_the_wait_is_bounded() {
        let waits: Vec<_> = (1..=9).map(Certificate::backoff).collect();
        assert_eq!(
            waits,
            [300, 600, 1200, 2400, 4800, 9600, 19_200, 19_200, 19_200]
        );
    }

    #[test]
    fn status_reflects_the_certificate_and_the_last_attempt() {
        let mut cert = certificate(None, None);
        assert_eq!(cert.status(0, false), Status::Pending);
        assert_eq!(cert.status(0, true), Status::Issuing);
        cert.last_error = Some("boom".into());
        assert_eq!(cert.status(0, false), Status::Failed);
        cert.not_after = Some(100);
        assert_eq!(
            cert.status(50, false),
            Status::Valid,
            "a failed renewal keeps serving"
        );
        assert_eq!(cert.status(100, false), Status::Expired);
    }
}
