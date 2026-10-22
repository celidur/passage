use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub admin: Admin,
    pub http: Http,
    /// Unset disables HTTPS.
    pub https: Option<Https>,
    pub acme: Option<Acme>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    pub data_dir: PathBuf,
    /// How often upstream host names are looked up again.
    pub dns_refresh_secs: u64,
}

impl Default for General {
    fn default() -> Self {
        Self {
            data_dir: "./data".into(),
            dns_refresh_secs: 30,
        }
    }
}

/// Plain HTTP. Also answers ACME HTTP-01 challenges and redirects to HTTPS.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Http {
    pub listen: SocketAddr,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8080".parse().unwrap(),
        }
    }
}

/// Dashboard and API; always loopback.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Admin {
    pub listen: SocketAddr,
}

impl Default for Admin {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8081".parse().unwrap(),
        }
    }
}

/// HTTPS, with a certificate chosen per server name (SNI).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Https {
    pub listen: SocketAddr,
    /// Fallback certificate for unknown names and clients that send no server name.
    pub certificate: Option<PathBuf>,
    pub private_key: Option<PathBuf>,
}

/// Automatic certificates from an ACME certificate authority such as Let's Encrypt.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Acme {
    /// Contact address registered with the certificate authority.
    pub email: String,
    /// Must be set to accept the certificate authority's terms of service.
    pub terms_agreed: bool,
    pub directory_url: String,
    /// Trusted root for a private ACME server (for example Pebble or step-ca).
    pub ca_certificate: Option<PathBuf>,
    /// Program run as `hook present|cleanup <record name> <value>` to publish DNS-01 proofs.
    pub dns_hook: Option<PathBuf>,
    /// How long to wait after publishing a DNS record before asking the CA to check it.
    pub dns_propagation_secs: u64,
    /// Renewal starts this long before expiry (or at two thirds of a short-lived certificate).
    pub renew_before_days: u64,
}

impl Default for Acme {
    fn default() -> Self {
        Self {
            email: String::new(),
            terms_agreed: false,
            directory_url: LETS_ENCRYPT.into(),
            ca_certificate: None,
            dns_hook: None,
            dns_propagation_secs: 30,
            renew_before_days: 30,
        }
    }
}

impl Acme {
    pub fn renew_before(&self) -> Duration {
        Duration::from_secs(self.renew_before_days * 86_400)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.email.contains('@') && !self.email.contains(char::is_whitespace),
            "acme.email must be a contact address"
        );
        ensure!(
            self.terms_agreed,
            "set acme.terms_agreed = true to accept the certificate authority's terms of service"
        );
        ensure!(
            self.directory_url.starts_with("https://"),
            "acme.directory_url must be an https:// URL"
        );
        ensure!(
            (1..=60).contains(&self.renew_before_days),
            "acme.renew_before_days must be between 1 and 60"
        );
        ensure!(
            self.dns_propagation_secs <= 600,
            "acme.dns_propagation_secs must be at most 600"
        );
        for (name, path) in [
            ("ca_certificate", &self.ca_certificate),
            ("dns_hook", &self.dns_hook),
        ] {
            if let Some(path) = path {
                ensure!(path.is_file(), "acme.{name} {} must exist", path.display());
            }
        }
        Ok(())
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let config: Self = toml::from_str(
            &std::fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?,
        )?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.admin.listen.ip().is_loopback(),
            "administration must bind to a loopback address; use an SSH tunnel"
        );
        ensure!(
            self.admin.listen.port() != 0 && self.http.listen.port() != 0,
            "listen ports must not be zero"
        );
        ensure!(
            self.admin.listen.port() != self.http.listen.port(),
            "[admin] listen and [http] listen must use different ports"
        );
        ensure!(
            (1..=86_400).contains(&self.general.dns_refresh_secs),
            "general.dns_refresh_secs must be between 1 and 86400"
        );
        if let Some(https) = &self.https {
            let port = https.listen.port();
            ensure!(
                port != 0 && port != self.admin.listen.port() && port != self.http.listen.port(),
                "[https] listen must use a separate nonzero port"
            );
            ensure!(
                https.certificate.is_some() == https.private_key.is_some(),
                "https.certificate and https.private_key must be set together"
            );
            for path in [&https.certificate, &https.private_key]
                .into_iter()
                .flatten()
            {
                ensure!(path.is_file(), "TLS file {} must exist", path.display());
            }
        }
        if let Some(acme) = &self.acme {
            ensure!(self.https.is_some(), "[acme] needs an [https] listener");
            acme.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    const HTTPS: &str = "[https]\nlisten = \"0.0.0.0:8443\"\n";

    #[test]
    fn https_is_off_unless_a_section_is_set() {
        let config = parse("").unwrap();
        assert!(config.https.is_none());
        assert_eq!(config.http.listen.port(), 8080);
        assert_eq!(config.admin.listen.port(), 8081);
        let config = parse(HTTPS).unwrap().https.unwrap();
        assert!(
            config.certificate.is_none(),
            "no fallback certificate is needed"
        );
        assert!(parse("[acme]\nemail = \"a@b.c\"\nterms_agreed = true\n").is_err());
    }

    #[test]
    fn listeners_need_distinct_ports() {
        assert!(parse("[https]\nlisten = \"0.0.0.0:8081\"\n").is_err());
        assert!(parse("[https]\nlisten = \"0.0.0.0:8080\"\n").is_err());
        assert!(parse("[http]\nlisten = \"0.0.0.0:8081\"\n").is_err());
    }

    #[test]
    fn the_fallback_certificate_needs_both_files() {
        let text = format!("{HTTPS}certificate = \"/nonexistent\"\n");
        assert!(parse(&text).is_err(), "the key is missing");
        let text =
            format!("{HTTPS}certificate = \"/nonexistent\"\nprivate_key = \"/nonexistent\"\n");
        assert!(parse(&text).is_err(), "the files must exist");
    }

    #[test]
    fn acme_requires_https_terms_and_a_contact() {
        let acme = "[acme]\nemail = \"ops@example.com\"\nterms_agreed = true\n";
        assert!(parse(&format!("{HTTPS}{acme}")).is_ok());
        assert!(parse(&format!("{HTTPS}[acme]\nemail = \"ops@example.com\"\n")).is_err());
        assert!(parse(&format!("{HTTPS}[acme]\nterms_agreed = true\n")).is_err());
        let insecure = format!("{acme}directory_url = \"http://ca.example/dir\"\n");
        assert!(parse(&format!("{HTTPS}{insecure}")).is_err());
    }

    #[test]
    fn unknown_acme_settings_are_rejected() {
        let text =
            format!("{HTTPS}[acme]\nemail = \"a@b.c\"\nterms_agreed = true\nstaging = true\n");
        assert!(parse(&text).is_err());
    }
}
