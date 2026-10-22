//! Persistence of managed certificates and small settings.

use super::Store;
use crate::certificates::Certificate;
use anyhow::Result;
use rusqlite::{OptionalExtension, params};

/// A certificate record with its PEM chain and key, once issued.
pub struct StoredCertificate {
    pub certificate: Certificate,
    pub material: Option<(String, String)>,
}

impl Store {
    pub fn certificates(&self) -> Result<Vec<StoredCertificate>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT document, chain_pem, key_pem FROM certificates
             ORDER BY json_extract(document, '$.created_at'), id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (document, chain, key) = row?;
            Ok(StoredCertificate {
                certificate: serde_json::from_str(&document)?,
                material: chain.zip(key),
            })
        })
        .collect()
    }

    pub fn certificate(&self, id: &str) -> Result<Option<Certificate>> {
        let document: Option<String> = self
            .conn()
            .query_row(
                "SELECT document FROM certificates WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(document.map(|d| serde_json::from_str(&d)).transpose()?)
    }

    /// Inserts or updates the record, leaving any stored key and chain untouched.
    pub fn save_certificate(&self, certificate: &Certificate) -> Result<()> {
        self.conn().execute(
            "INSERT INTO certificates(id, document) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET document = excluded.document",
            params![certificate.id, serde_json::to_string(certificate)?],
        )?;
        Ok(())
    }

    /// Stores a freshly issued chain and key together with the updated record.
    pub fn save_material(
        &self,
        certificate: &Certificate,
        chain_pem: &str,
        key_pem: &str,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE certificates SET document = ?2, chain_pem = ?3, key_pem = ?4 WHERE id = ?1",
            params![
                certificate.id,
                serde_json::to_string(certificate)?,
                chain_pem,
                key_pem
            ],
        )?;
        Ok(())
    }

    pub fn delete_certificate(&self, id: &str) -> Result<bool> {
        let deleted = self
            .conn()
            .execute("DELETE FROM certificates WHERE id = ?1", [id])?;
        Ok(deleted > 0)
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certificates::Challenge;

    fn certificate(id: &str) -> Certificate {
        Certificate {
            id: id.into(),
            domains: vec![format!("{id}.example.com")],
            challenge: Challenge::Http01,
            created_at: 1,
            not_before: None,
            not_after: None,
            last_error: None,
            failures: 0,
            next_attempt: None,
            renew_requested: false,
        }
    }

    #[test]
    fn certificates_keep_their_material_across_record_updates() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.sqlite")).unwrap();
        let mut cert = certificate("a");
        store.save_certificate(&cert).unwrap();
        assert!(store.certificates().unwrap()[0].material.is_none());

        cert.not_after = Some(10);
        store.save_material(&cert, "CHAIN", "KEY").unwrap();
        cert.last_error = Some("renewal failed".into());
        store.save_certificate(&cert).unwrap();

        let stored = store.certificates().unwrap();
        assert_eq!(stored[0].material, Some(("CHAIN".into(), "KEY".into())));
        assert_eq!(
            stored[0].certificate.last_error.as_deref(),
            Some("renewal failed")
        );
        assert!(store.certificate("a").unwrap().is_some());
        assert!(store.delete_certificate("a").unwrap());
        assert!(!store.delete_certificate("a").unwrap());
        assert!(store.certificate("a").unwrap().is_none());
    }

    #[test]
    fn settings_are_replaced_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.sqlite")).unwrap();
        assert!(store.setting("k").unwrap().is_none());
        store.set_setting("k", "one").unwrap();
        store.set_setting("k", "two").unwrap();
        assert_eq!(store.setting("k").unwrap().as_deref(), Some("two"));
    }
}
