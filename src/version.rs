//! Reading and bumping the project version in its manifest.

use crate::config::{Bump, Preset, VersionCfg};
use anyhow::{Context, Result, bail};
use regex::Regex;
use std::path::{Path, PathBuf};

pub struct VersionFile {
    pub path: PathBuf,
    re: Regex,
}

impl VersionFile {
    pub fn new(cfg: &VersionCfg) -> Result<Self> {
        let (default_file, pattern) = match cfg.preset {
            // Several targets each carry MARKETING_VERSION (test targets often
            // sit at 1.0); the first one is the app's.
            Preset::Xcode => (None, r"MARKETING_VERSION = ([^;\s]+);"),
            Preset::Cargo => (Some("Cargo.toml"), r#"(?m)^version\s*=\s*"([^"]+)""#),
            Preset::Cmake => (Some("CMakeLists.txt"), r#"(?m)^set\(APP_VERSION "([^"]+)"\)"#),
            Preset::Regex => (None, cfg.regex.as_deref().unwrap_or_default()),
        };
        let path = cfg
            .file
            .clone()
            .or(default_file.map(PathBuf::from))
            .context("version.file is required for this preset")?;
        let re = Regex::new(pattern).with_context(|| format!("invalid version regex: {pattern}"))?;
        if re.captures_len() < 2 {
            bail!("version regex needs a capture group: {pattern}");
        }
        Ok(Self { path, re })
    }

    pub fn read(&self, root: &Path) -> Result<String> {
        let text = self.load(root)?;
        read_from(&self.re, &text)
            .with_context(|| format!("could not read the version from {}", self.path.display()))
    }

    /// Replace every occurrence whose captured version equals `from` with `to`.
    pub fn write(&self, root: &Path, from: &str, to: &str) -> Result<()> {
        let text = self.load(root)?;
        let (new, count) = replace_in(&self.re, &text, from, to);
        if count == 0 {
            bail!("version {from} not found in {}", self.path.display());
        }
        std::fs::write(root.join(&self.path), new)?;
        Ok(())
    }

    fn load(&self, root: &Path) -> Result<String> {
        std::fs::read_to_string(root.join(&self.path))
            .with_context(|| format!("cannot read {}", self.path.display()))
    }
}

fn read_from(re: &Regex, text: &str) -> Option<String> {
    re.captures(text).map(|c| c[1].trim().to_string()).filter(|v| !v.is_empty())
}

fn replace_in(re: &Regex, text: &str, from: &str, to: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut count = 0;
    for caps in re.captures_iter(text) {
        let m = caps.get(1).unwrap();
        if m.as_str().trim() == from {
            out.push_str(&text[last..m.start()]);
            out.push_str(to);
            last = m.end();
            count += 1;
        }
    }
    out.push_str(&text[last..]);
    (out, count)
}

/// Bump a dotted X.Y.Z version. Missing components count as 0.
pub fn bump(version: &str, how: Bump) -> Result<String> {
    let parts: Vec<u64> = version
        .split('.')
        .map(|p| p.parse::<u64>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("cannot bump non-numeric version '{version}'; use --version"))?;
    let get = |i: usize| parts.get(i).copied().unwrap_or(0);
    let (maj, min, pat) = (get(0), get(1), get(2));
    Ok(match how {
        Bump::Major => format!("{}.0.0", maj + 1),
        Bump::Minor => format!("{maj}.{}.0", min + 1),
        Bump::Patch => format!("{maj}.{min}.{}", pat + 1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vf(preset: Preset, regex: Option<&str>) -> VersionFile {
        VersionFile::new(&VersionCfg {
            preset,
            file: Some("f".into()),
            regex: regex.map(String::from),
        })
        .unwrap()
    }

    const PBXPROJ: &str = "\
				CURRENT_PROJECT_VERSION = 0;
				MARKETING_VERSION = 1.8.0;
				CURRENT_PROJECT_VERSION = 0;
				MARKETING_VERSION = 1.8.0;
				MARKETING_VERSION = 1.0;
				MARKETING_VERSION = 1.0;
";

    #[test]
    fn xcode_reads_first_and_bumps_only_matching() {
        let v = vf(Preset::Xcode, None);
        assert_eq!(read_from(&v.re, PBXPROJ).unwrap(), "1.8.0");
        let (new, n) = replace_in(&v.re, PBXPROJ, "1.8.0", "1.9.0");
        assert_eq!(n, 2);
        assert_eq!(new.matches("MARKETING_VERSION = 1.9.0;").count(), 2);
        assert_eq!(new.matches("MARKETING_VERSION = 1.0;").count(), 2);
    }

    #[test]
    fn cargo_ignores_dependency_versions() {
        let v = vf(Preset::Cargo, None);
        let toml = "[package]\nname = \"sde\"\nversion = \"0.3.6\"\n\n[dependencies]\nfoo = { version = \"0.3.6\" }\n";
        assert_eq!(read_from(&v.re, toml).unwrap(), "0.3.6");
        let (new, n) = replace_in(&v.re, toml, "0.3.6", "0.3.7");
        assert_eq!(n, 1);
        assert!(new.contains("version = \"0.3.7\"\n"));
        assert!(new.contains("foo = { version = \"0.3.6\" }"));
    }

    #[test]
    fn cmake() {
        let v = vf(Preset::Cmake, None);
        let text = "project(X)\nset(APP_VERSION \"0.8.0\")\n";
        assert_eq!(read_from(&v.re, text).unwrap(), "0.8.0");
        assert_eq!(replace_in(&v.re, text, "0.8.0", "0.9.0").0, "project(X)\nset(APP_VERSION \"0.9.0\")\n");
    }

    #[test]
    fn regex_preset() {
        let v = vf(Preset::Regex, Some(r"(?m)^pluginVersion\s*=\s*(\S+)"));
        assert_eq!(read_from(&v.re, "a=1\npluginVersion = 2.1.0\n").unwrap(), "2.1.0");
    }

    #[test]
    fn bumps() {
        assert_eq!(bump("1.8.0", Bump::Minor).unwrap(), "1.9.0");
        assert_eq!(bump("0.3.6", Bump::Patch).unwrap(), "0.3.7");
        assert_eq!(bump("1.8.3", Bump::Major).unwrap(), "2.0.0");
        assert_eq!(bump("1.0", Bump::Patch).unwrap(), "1.0.1");
        assert!(bump("1.0-beta", Bump::Patch).is_err());
    }
}
