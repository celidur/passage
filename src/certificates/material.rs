//! Keys, signing requests and certificate parsing with OpenSSL.

use anyhow::{Context, Result, ensure};
use openssl::{
    asn1::{Asn1Time, Asn1TimeRef},
    hash::MessageDigest,
    pkey::{PKey, Private},
    stack::Stack,
    x509::{X509, X509Name, X509Req, extension::SubjectAlternativeName},
};

/// A leaf certificate, its intermediates and the private key, ready for a TLS handshake.
pub struct CertifiedKey {
    pub leaf: X509,
    pub chain: Vec<X509>,
    pub key: PKey<Private>,
}

pub struct Loaded {
    pub certified: CertifiedKey,
    pub not_before: i64,
    pub not_after: i64,
}

/// A fresh P-256 key and a DER signing request naming `domains`. The subject stays empty because
/// certificate authorities read the names from the SAN extension, and a CN is limited to 64 bytes.
pub fn new_key_and_csr(domains: &[String]) -> Result<(String, Vec<u8>)> {
    let key = PKey::ec_gen("prime256v1")?;
    let mut request = X509Req::builder()?;
    request.set_version(0)?;
    request.set_subject_name(&X509Name::builder()?.build())?;
    request.set_pubkey(&key)?;
    let mut names = SubjectAlternativeName::new();
    for domain in domains {
        names.dns(domain);
    }
    let mut extensions = Stack::new()?;
    extensions.push(names.build(&request.x509v3_context(None))?)?;
    request.add_extensions(&extensions)?;
    request.sign(&key, MessageDigest::sha256())?;
    Ok((
        String::from_utf8(key.private_key_to_pem_pkcs8()?)?,
        request.build().to_der()?,
    ))
}

/// Parses a PEM chain (leaf first) and its key, checking that they belong together.
pub fn load(chain_pem: &str, key_pem: &str) -> Result<Loaded> {
    let mut chain =
        X509::stack_from_pem(chain_pem.as_bytes()).context("invalid certificate chain")?;
    ensure!(!chain.is_empty(), "the certificate chain is empty");
    let leaf = chain.remove(0);
    let key = PKey::private_key_from_pem(key_pem.as_bytes()).context("invalid private key")?;
    ensure!(
        leaf.public_key()?.public_eq(&key),
        "the private key does not match the certificate"
    );
    Ok(Loaded {
        not_before: unix(leaf.not_before())?,
        not_after: unix(leaf.not_after())?,
        certified: CertifiedKey { leaf, chain, key },
    })
}

fn unix(time: &Asn1TimeRef) -> Result<i64> {
    let since_epoch = Asn1Time::from_unix(0)?.diff(time)?;
    Ok(i64::from(since_epoch.days) * 86_400 + i64::from(since_epoch.secs))
}

#[cfg(test)]
pub(crate) mod testing {
    use openssl::{
        asn1::Asn1Time,
        bn::BigNum,
        hash::MessageDigest,
        pkey::PKey,
        x509::{X509, X509NameBuilder, extension::SubjectAlternativeName},
    };

    /// A self-signed certificate for `names`, valid from now for `days`, as (chain PEM, key PEM).
    pub fn self_signed(names: &[&str], days: u32) -> (String, String) {
        let key = PKey::ec_gen("prime256v1").unwrap();
        let mut subject = X509NameBuilder::new().unwrap();
        subject.append_entry_by_text("CN", names[0]).unwrap();
        let subject = subject.build();
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        let serial = BigNum::from_u32(rand::random::<u32>() | 1).unwrap();
        cert.set_serial_number(&serial.to_asn1_integer().unwrap())
            .unwrap();
        cert.set_subject_name(&subject).unwrap();
        cert.set_issuer_name(&subject).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        cert.set_not_after(&Asn1Time::days_from_now(days).unwrap())
            .unwrap();
        let mut san = SubjectAlternativeName::new();
        for name in names {
            san.dns(name);
        }
        let san = san.build(&cert.x509v3_context(None, None)).unwrap();
        cert.append_extension(san).unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();
        (
            String::from_utf8(cert.build().to_pem().unwrap()).unwrap(),
            String::from_utf8(key.private_key_to_pem_pkcs8().unwrap()).unwrap(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testing::self_signed;

    #[test]
    fn signing_request_names_every_domain_and_is_signed() {
        let domains = ["app.example.com".to_string(), "*.example.com".to_string()];
        let (key_pem, der) = new_key_and_csr(&domains).unwrap();
        let request = X509Req::from_der(&der).unwrap();
        assert!(request.verify(&request.public_key().unwrap()).unwrap());
        let text = String::from_utf8(request.to_text().unwrap()).unwrap();
        assert!(text.contains("DNS:app.example.com") && text.contains("DNS:*.example.com"));
        assert!(PKey::private_key_from_pem(key_pem.as_bytes()).is_ok());
    }

    #[test]
    fn loading_reads_the_validity_window() {
        let (chain, key) = self_signed(&["app.example.com"], 10);
        let loaded = load(&chain, &key).unwrap();
        let lifetime = loaded.not_after - loaded.not_before;
        assert!((lifetime - 10 * 86_400).abs() <= 1, "{lifetime}");
        let now = Asn1Time::days_from_now(0).unwrap();
        let expected = unix(&now).unwrap();
        assert!((loaded.not_before - expected).abs() < 5);
    }

    #[test]
    fn a_key_from_another_certificate_is_rejected() {
        let (chain, _) = self_signed(&["a.example.com"], 1);
        let (_, other_key) = self_signed(&["b.example.com"], 1);
        assert!(load(&chain, &other_key).is_err());
        assert!(load("not a certificate", &other_key).is_err());
    }
}
