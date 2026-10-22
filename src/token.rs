use anyhow::{Context, Result, ensure};
use rand::Rng;
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};
use subtle::ConstantTimeEq;

/// The administrator bearer token. Only its SHA-256 digest is kept in memory.
pub struct AdminToken {
    digest: [u8; 32],
}

impl AdminToken {
    /// Writes a new random token readable only by its owner. Never overwrites an existing file.
    pub fn generate(path: &Path) -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .context("administrator token already exists or cannot be created")?;
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        writeln!(file, "{}", hex::encode(bytes))?;
        file.sync_all()?;
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path).context("run `passage init` before starting")?;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "{} must not be readable by group or others (chmod 600)",
            path.display()
        );
        let token = std::fs::read_to_string(path)?;
        let token = token.trim();
        ensure!(
            token.len() == 64 && hex::decode(token).is_ok(),
            "invalid administrator token file"
        );
        Ok(Self {
            digest: digest(token),
        })
    }

    pub fn verify(&self, candidate: &str) -> bool {
        self.digest.ct_eq(&digest(candidate)).into()
    }
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_token_verifies_and_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("admin.token");
        AdminToken::generate(&path).unwrap();
        assert!(AdminToken::generate(&path).is_err());
        let raw = std::fs::read_to_string(&path).unwrap();
        let token = AdminToken::read(&path).unwrap();
        assert!(token.verify(raw.trim()));
        assert!(!token.verify("wrong"));
    }

    #[test]
    fn group_readable_token_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("admin.token");
        AdminToken::generate(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(AdminToken::read(&path).is_err());
    }
}
