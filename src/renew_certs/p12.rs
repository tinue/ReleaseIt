//! Checks on a `.p12` before it goes to CI, via the system's openssl
//! (LibreSSL reads Keychain-exported PKCS#12 without `-legacy`). The password
//! is handed over in the environment, never on the command line.

use crate::cert::CertInfo;
use anyhow::{Context, Result, bail};
use base64::Engine;
use std::io::Write;
use std::process::{Command, Stdio};

const PW_ENV: &str = "RENEW_CERTS_OPENSSL_PASSIN";

fn openssl_path() -> String {
    std::env::var("RENEW_CERTS_OPENSSL").unwrap_or_else(|_| "/usr/bin/openssl".into())
}

/// Run openssl with optional stdin and password; return stdout.
fn openssl(args: &[&str], stdin: &[u8], password: Option<&str>) -> Result<String> {
    let mut cmd = Command::new(openssl_path());
    cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(pw) = password {
        cmd.env(PW_ENV, pw);
    }
    let mut child = cmd.spawn().context("cannot run openssl")?;
    child.stdin.take().context("stdin")?.write_all(stdin)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("openssl {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// DER contents of every `-----BEGIN <label>-----` block in `pem`.
pub fn pem_blocks(pem: &str, label: &str) -> Vec<Vec<u8>> {
    let (begin, end) = (format!("-----BEGIN {label}-----"), format!("-----END {label}-----"));
    let mut out = Vec::new();
    let mut rest = pem;
    while let Some(i) = rest.find(&begin) {
        let body = &rest[i + begin.len()..];
        let Some(j) = body.find(&end) else { break };
        let b64: String = body[..j].chars().filter(|c| !c.is_whitespace()).collect();
        if let Ok(der) = base64::engine::general_purpose::STANDARD.decode(b64) {
            out.push(der);
        }
        rest = &body[j + end.len()..];
    }
    out
}

/// Open the `.p12` with `password` and require exactly one end-entity
/// certificate plus the private key belonging to it; return the cert.
pub fn inspect(p12: &[u8], password: &str) -> Result<CertInfo> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("in.p12");
    std::fs::write(&path, p12)?;
    let p = path.to_str().context("temp path")?;
    let passin = format!("env:{PW_ENV}");

    let keys_pem = openssl(&["pkcs12", "-in", p, "-passin", &passin, "-nocerts", "-nodes"], b"", Some(password))
        .context("cannot open the .p12 (wrong password?)")?;
    let keys = keys_pem.lines().filter(|l| l.starts_with("-----BEGIN") && l.contains("PRIVATE KEY")).count();
    if keys != 1 {
        bail!(
            "the .p12 holds {keys} private keys; export the identity (certificate with its key), not just the certificate"
        );
    }
    let certs_pem = openssl(&["pkcs12", "-in", p, "-passin", &passin, "-nokeys", "-clcerts"], b"", Some(password))?;
    let certs = pem_blocks(&certs_pem, "CERTIFICATE");
    if certs.len() != 1 {
        bail!("the .p12 holds {} certificates; export exactly one identity", certs.len());
    }
    let cert_pub = openssl(&["x509", "-pubkey", "-noout"], certs_pem.as_bytes(), None)?;
    let key_pub = openssl(&["pkey", "-pubout"], keys_pem.as_bytes(), None)?;
    if cert_pub.trim() != key_pub.trim() {
        bail!("the private key in the .p12 does not belong to its certificate");
    }
    CertInfo::from_der(&certs[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert::Kind;

    const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/certs");

    #[test]
    fn inspects_a_good_p12() {
        let c = inspect(&std::fs::read(format!("{FIX}/application-g2.p12")).unwrap(), "test").unwrap();
        assert_eq!(c.kind, Some(Kind::Application));
        assert!(c.g2);
    }

    #[test]
    fn wrong_password() {
        let e = inspect(&std::fs::read(format!("{FIX}/application-g2.p12")).unwrap(), "nope").unwrap_err();
        assert!(format!("{e:#}").contains("wrong password"), "{e:#}");
    }

    #[test]
    fn cert_only() {
        let e = inspect(&std::fs::read(format!("{FIX}/application-certonly.p12")).unwrap(), "test").unwrap_err();
        assert!(format!("{e:#}").contains("0 private keys"), "{e:#}");
    }
}
