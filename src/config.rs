use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub proxy_listen: SocketAddr,
    pub admin_listen: SocketAddr,
    pub data_dir: PathBuf,
    /// How often upstream host names are looked up again.
    pub dns_refresh_secs: u64,
    pub tls: Option<Tls>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tls {
    pub listen: SocketAddr,
    pub certificate: PathBuf,
    pub private_key: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            proxy_listen: "0.0.0.0:8081".parse().unwrap(),
            admin_listen: "127.0.0.1:8080".parse().unwrap(),
            data_dir: "./data".into(),
            dns_refresh_secs: 30,
            tls: None,
        }
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
            self.admin_listen.ip().is_loopback(),
            "administration must bind to a loopback address; use an SSH tunnel"
        );
        ensure!(
            self.admin_listen.port() != 0 && self.proxy_listen.port() != 0,
            "listen ports must not be zero"
        );
        ensure!(
            self.admin_listen.port() != self.proxy_listen.port(),
            "admin and proxy ports must differ"
        );
        ensure!(
            (1..=86_400).contains(&self.dns_refresh_secs),
            "dns_refresh_secs must be between 1 and 86400"
        );
        if let Some(tls) = &self.tls {
            ensure!(
                tls.listen.port() != 0
                    && tls.listen.port() != self.admin_listen.port()
                    && tls.listen.port() != self.proxy_listen.port(),
                "TLS listener must use a separate nonzero port"
            );
            ensure!(
                tls.certificate.is_file() && tls.private_key.is_file(),
                "TLS certificate/key files must exist"
            );
        }
        Ok(())
    }
}
