//! `~/.config/releaseit/certs.yaml`: which repos get the signing secrets.

use crate::cert::Kind;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const SAMPLE: &str = "\
team_id: ABCDE12345
repos:
  - owner/RepoOne
  - owner/RepoTwo
# optional:
# min_days: 30
# secrets:
#   application: APPLE_CERT_APPLICATION_P12_BASE64
#   installer:   APPLE_CERT_INSTALLER_P12_BASE64
#   password:    APPLE_CERT_P12_PASSWORD
";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Apple team ID; the certificate subject's OU must match.
    pub team_id: String,
    /// GitHub repos ("owner/name") whose secrets are rotated.
    pub repos: Vec<String>,
    /// Refuse (and `status` flags) certs expiring within this many days.
    #[serde(default = "default_min_days")]
    pub min_days: i64,
    #[serde(default)]
    pub secrets: Secrets,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Secrets {
    pub application: String,
    pub installer: String,
    pub password: String,
}

impl Default for Secrets {
    fn default() -> Self {
        Self {
            application: "APPLE_CERT_APPLICATION_P12_BASE64".into(),
            installer: "APPLE_CERT_INSTALLER_P12_BASE64".into(),
            password: "APPLE_CERT_P12_PASSWORD".into(),
        }
    }
}

fn default_min_days() -> i64 {
    30
}

impl Config {
    pub fn default_path() -> Result<PathBuf> {
        let home = std::env::var_os("HOME").context("HOME not set")?;
        Ok(PathBuf::from(home).join(".config/releaseit/certs.yaml"))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|_| anyhow::anyhow!("{} not found. Create it, e.g.:\n\n{SAMPLE}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let cfg: Config = serde_yaml_ng::from_str(text)?;
        if cfg.team_id.trim().is_empty() {
            bail!("team_id is empty");
        }
        if cfg.repos.is_empty() {
            bail!("repos is empty");
        }
        if let Some(r) = cfg.repos.iter().find(|r| r.split('/').count() != 2) {
            bail!("repo '{r}' is not of the form owner/name");
        }
        Ok(cfg)
    }

    pub fn secret(&self, kind: Kind) -> &str {
        match kind {
            Kind::Application => &self.secrets.application,
            Kind::Installer => &self.secrets.installer,
        }
    }

    /// Repo variables recording what a secret currently holds, so `status`
    /// can report it (secrets can't be read back):
    /// APPLE_CERT_APPLICATION_P12_BASE64 → APPLE_CERT_APPLICATION_SHA1 / _EXPIRES.
    pub fn variables(&self, kind: Kind) -> (String, String) {
        let s = self.secret(kind);
        let base = s.strip_suffix("_P12_BASE64").unwrap_or(s);
        (format!("{base}_SHA1"), format!("{base}_EXPIRES"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::parse("team_id: T\nrepos: [a/b]\n").unwrap();
        assert_eq!(c.min_days, 30);
        assert_eq!(c.secret(Kind::Installer), "APPLE_CERT_INSTALLER_P12_BASE64");
        assert_eq!(
            c.variables(Kind::Application),
            ("APPLE_CERT_APPLICATION_SHA1".into(), "APPLE_CERT_APPLICATION_EXPIRES".into())
        );
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Config::parse("team_id: T\nrepos: []\n").is_err());
        assert!(Config::parse("team_id: T\nrepos: [nameonly]\n").is_err());
        assert!(Config::parse("team_id: T\nrepos: [a/b]\nunknown: 1\n").is_err());
    }

    #[test]
    fn sample_parses() {
        Config::parse(SAMPLE).unwrap();
    }
}
