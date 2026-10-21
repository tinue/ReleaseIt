//! Developer ID certificate facts and the checks a renewal must pass.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use sha1::{Digest, Sha1};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Kind {
    Application,
    Installer,
}

pub const KINDS: [Kind; 2] = [Kind::Application, Kind::Installer];

impl Kind {
    /// Common-name prefix Apple uses for this certificate type.
    pub fn cn_prefix(self) -> &'static str {
        match self {
            Kind::Application => "Developer ID Application:",
            Kind::Installer => "Developer ID Installer:",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Application => "Developer ID Application",
            Kind::Installer => "Developer ID Installer",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CertInfo {
    pub kind: Option<Kind>,
    pub common_name: String,
    /// Subject OU: the Apple team ID.
    pub team: String,
    /// Issued by the G2 Developer ID Sub-CA (issuer OU "G2"). The original
    /// Sub-CA expires 2027-02-01 and takes every certificate it issued with it;
    /// Xcode's "Manage Certificates" still issued from it in October 2026.
    pub g2: bool,
    pub not_after: DateTime<Utc>,
    /// Upper-case hex, as `security find-identity` prints it.
    pub sha1: String,
}

impl CertInfo {
    pub fn from_der(der: &[u8]) -> Result<Self> {
        let (_, cert) = x509_parser::parse_x509_certificate(der).context("not an X.509 certificate")?;
        let first = |it: &mut dyn Iterator<Item = &x509_parser::x509::AttributeTypeAndValue>| {
            it.next().and_then(|a| a.as_str().ok()).unwrap_or("").to_string()
        };
        let common_name = first(&mut cert.subject().iter_common_name());
        let team = first(&mut cert.subject().iter_organizational_unit());
        let g2 = cert.issuer().iter_organizational_unit().any(|a| a.as_str() == Ok("G2"));
        let not_after = DateTime::from_timestamp(cert.validity().not_after.timestamp(), 0).context("bad notAfter")?;
        let kind = KINDS.into_iter().find(|k| common_name.starts_with(k.cn_prefix()));
        let sha1 = Sha1::digest(der).iter().map(|b| format!("{b:02X}")).collect();
        Ok(Self { kind, common_name, team, g2, not_after, sha1 })
    }

    pub fn short_sha1(&self) -> &str {
        short(&self.sha1)
    }

    pub fn expires(&self) -> String {
        self.not_after.format("%Y-%m-%d").to_string()
    }

    pub fn describe(&self) -> String {
        format!("{}…  {}  expires {}", self.short_sha1(), if self.g2 { "G2    " } else { "old CA" }, self.expires())
    }
}

/// The first 8 hex digits of a SHA-1, for display.
pub fn short(sha1: &str) -> &str {
    &sha1[..8.min(sha1.len())]
}

/// Everything a certificate must satisfy before it goes to CI.
pub struct Rules<'a> {
    pub kind: Kind,
    pub team: &'a str,
    pub min_days: i64,
    pub now: DateTime<Utc>,
    /// Latest expiry CI already has for this kind (from the repo variables).
    pub ci_expires: Option<DateTime<Utc>>,
    pub force: bool,
}

impl Rules<'_> {
    /// The problems with `c`, empty when it may be uploaded.
    pub fn problems(&self, c: &CertInfo) -> Vec<String> {
        let mut p = Vec::new();
        if c.kind != Some(self.kind) {
            p.push(format!("'{}' is not a {} certificate", c.common_name, self.kind.name()));
        }
        if c.team != self.team {
            p.push(format!("team '{}' does not match team_id '{}'", c.team, self.team));
        }
        if !c.g2 {
            p.push(
                "issued by the original Developer ID CA, which expires 2027-02-01; \
                 create it in the developer portal with the G2 Sub-CA (see `renew-certs guide`)"
                    .into(),
            );
        }
        let days = (c.not_after - self.now).num_days();
        if days < self.min_days {
            p.push(format!("expires {} ({days} days), min_days is {}", c.expires(), self.min_days));
        }
        if let Some(ci) = self.ci_expires
            && c.not_after < ci
            && !self.force
        {
            p.push(format!(
                "expires {} but CI already has one until {} (--force to downgrade)",
                c.expires(),
                ci.format("%Y-%m-%d")
            ));
        }
        p
    }

    pub fn check(&self, c: &CertInfo) -> Result<()> {
        let p = self.problems(c);
        if !p.is_empty() {
            bail!("{} {} rejected:\n  - {}", self.kind.name(), c.short_sha1(), p.join("\n  - "));
        }
        Ok(())
    }
}

/// The keychain identity to renew with: the latest-expiring one of `kind`
/// that is G2 and from `team`. Old-CA and foreign certs are never picked.
pub fn pick<'a, T: AsRef<CertInfo>>(items: &'a [T], kind: Kind, team: &str) -> Option<&'a T> {
    items
        .iter()
        .filter(|i| {
            let c = i.as_ref();
            c.kind == Some(kind) && c.team == team && c.g2
        })
        .max_by_key(|i| i.as_ref().not_after)
}

impl AsRef<CertInfo> for CertInfo {
    fn as_ref(&self) -> &CertInfo {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn cert(kind: Kind, g2: bool, until: &str, sha1: &str) -> CertInfo {
        CertInfo {
            kind: Some(kind),
            common_name: format!("{} Someone (TEAM)", kind.cn_prefix()),
            team: "TEAM".into(),
            g2,
            not_after: at(until),
            sha1: sha1.into(),
        }
    }

    fn rules(kind: Kind) -> Rules<'static> {
        Rules { kind, team: "TEAM", min_days: 30, now: at("2026-10-21T00:00:00Z"), ci_expires: None, force: false }
    }

    #[test]
    fn accepts_a_fresh_g2_cert() {
        assert!(
            rules(Kind::Application).problems(&cert(Kind::Application, true, "2027-11-20T05:21:30Z", "A")).is_empty()
        );
    }

    #[test]
    fn rejects_old_ca_wrong_kind_team_and_near_expiry() {
        let r = rules(Kind::Installer);
        let mut c = cert(Kind::Application, false, "2026-11-01T00:00:00Z", "A");
        c.team = "OTHER".into();
        let p = r.problems(&c);
        assert_eq!(p.len(), 4, "{p:?}");
    }

    #[test]
    fn downgrade_needs_force() {
        let mut r = rules(Kind::Application);
        r.ci_expires = Some(at("2028-01-01T00:00:00Z"));
        let c = cert(Kind::Application, true, "2027-11-20T00:00:00Z", "A");
        assert_eq!(r.problems(&c).len(), 1);
        r.force = true;
        assert!(r.problems(&c).is_empty());
    }

    #[test]
    fn pick_skips_old_ca_and_takes_latest() {
        // Today's keychain: two old-CA certs (one an accidental Xcode one,
        // issued the same day as the G2 one) plus the G2 cert.
        let certs = [
            cert(Kind::Application, false, "2027-02-01T22:12:15Z", "07FC"),
            cert(Kind::Application, false, "2027-02-01T22:12:15Z", "8981"),
            cert(Kind::Application, true, "2027-11-20T05:21:30Z", "47C2"),
            cert(Kind::Installer, true, "2027-11-20T05:31:05Z", "1261"),
        ];
        assert_eq!(pick(&certs, Kind::Application, "TEAM").unwrap().sha1, "47C2");
        assert_eq!(pick(&certs, Kind::Installer, "TEAM").unwrap().sha1, "1261");
        assert!(pick(&certs, Kind::Installer, "OTHER").is_none());
    }

    #[test]
    fn parses_fixture_certs() {
        let g2 = CertInfo::from_der(&pem_der(include_str!("../../tests/fixtures/certs/application-g2.pem"))).unwrap();
        assert_eq!(g2.kind, Some(Kind::Application));
        assert_eq!(g2.team, "TESTTEAM01");
        assert!(g2.g2);
        assert_eq!(g2.sha1.len(), 40);
        let old = CertInfo::from_der(&pem_der(include_str!("../../tests/fixtures/certs/installer-oldca.pem"))).unwrap();
        assert_eq!(old.kind, Some(Kind::Installer));
        assert!(!old.g2);
    }

    fn pem_der(pem: &str) -> Vec<u8> {
        x509_parser::pem::Pem::iter_from_buffer(pem.as_bytes()).next().unwrap().unwrap().contents
    }
}
