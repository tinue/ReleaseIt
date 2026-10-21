//! Checks on a `.p12` before it goes to CI, via the system's openssl
//! (LibreSSL reads Keychain-exported PKCS#12 without `-legacy`). The password
//! is handed over in the environment, never on the command line.

use crate::cert::CertInfo;
use anyhow::{Context, Result, bail};
use releaseit::sh;
use x509_parser::pem::Pem;

const PW_ENV: &str = "RENEW_CERTS_OPENSSL_PASSIN";

fn openssl() -> String {
    std::env::var("RENEW_CERTS_OPENSSL").unwrap_or_else(|_| "/usr/bin/openssl".into())
}

fn pem_blocks(pem: &str) -> Result<Vec<Pem>> {
    Pem::iter_from_buffer(pem.as_bytes()).collect::<Result<_, _>>().context("unreadable openssl output")
}

/// Open the `.p12` with `password` and require exactly one end-entity
/// certificate plus the private key belonging to it; return the cert.
pub fn inspect(p12: &[u8], password: &str) -> Result<CertInfo> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("in.p12");
    std::fs::write(&path, p12)?;
    let openssl = openssl();

    // -clcerts: only certificates paired with a key (no CA chain).
    let passin = format!("env:{PW_ENV}");
    let args = ["pkcs12", "-in", sh::path_str(&path)?, "-passin", &passin, "-nodes", "-clcerts"];
    let pem = sh::run_with_stdin(&openssl, &args, b"", &[(PW_ENV, password)])
        .context("cannot open the .p12 (wrong password?)")?;
    let blocks = pem_blocks(&pem)?;
    let keys: Vec<&Pem> = blocks.iter().filter(|b| b.label.ends_with("PRIVATE KEY")).collect();
    let [key] = keys.as_slice() else {
        bail!(
            "the .p12 holds {} private keys; export the identity (certificate with its key), not just the certificate",
            keys.len()
        );
    };
    let certs: Vec<&Pem> = blocks.iter().filter(|b| b.label == "CERTIFICATE").collect();
    let [cert] = certs.as_slice() else {
        bail!("the .p12 holds {} certificates; export exactly one identity", certs.len());
    };

    let key_pub = sh::run_with_stdin(&openssl, &["pkey", "-inform", "DER", "-pubout"], &key.contents, &[])?;
    let key_spki = pem_blocks(&key_pub)?.into_iter().next().context("no public key from openssl")?.contents;
    let x509 = cert.parse_x509().context("not an X.509 certificate")?;
    if x509.public_key().raw != key_spki.as_slice() {
        bail!("the private key in the .p12 does not belong to its certificate");
    }
    CertInfo::from_der(&cert.contents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert::Kind;

    const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/certs");

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{FIX}/{name}.p12")).unwrap()
    }

    #[test]
    fn inspects_a_good_p12() {
        let c = inspect(&fixture("application-g2"), "test").unwrap();
        assert_eq!(c.kind, Some(Kind::Application));
        assert!(c.g2);
    }

    #[test]
    fn wrong_password() {
        let e = inspect(&fixture("application-g2"), "nope").unwrap_err();
        assert!(format!("{e:#}").contains("wrong password"), "{e:#}");
    }

    #[test]
    fn cert_only() {
        let e = inspect(&fixture("application-certonly"), "test").unwrap_err();
        assert!(format!("{e:#}").contains("0 private keys"), "{e:#}");
    }
}
