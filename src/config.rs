//! `release.yaml` — one per project, at the repository root.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "release.yaml";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Product name, used in release titles and DMG names.
    pub name: String,
    #[serde(default = "default_main_branch")]
    pub main_branch: String,
    pub version: VersionCfg,
    #[serde(default)]
    pub changelog: ChangelogCfg,
    #[serde(default)]
    pub next: NextCfg,
    /// Shell commands run before a pre-release and in --build-only
    /// (e.g. clippy/build/test). Any failure aborts.
    #[serde(default)]
    pub checks: Vec<String>,
    #[serde(default)]
    pub release: ReleaseCfg,
    /// Build steps; their assets are merged into one GitHub pre-release.
    #[serde(default)]
    pub build: Vec<BuildStep>,
    pub appstore: Option<AppStoreCfg>,
    /// Developer ID certificates CI signs with. Only `renew-certs` reads it;
    /// `release` parses it so that `deny_unknown_fields` still applies.
    #[allow(dead_code)]
    pub certs: Option<CertsCfg>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // read by renew-certs
pub struct CertsCfg {
    /// Apple team ID; the certificate subject's OU must match.
    pub team_id: String,
    /// Refuse (and `renew-certs status` flags) certificates expiring within
    /// this many days.
    #[serde(default = "default_min_days")]
    pub min_days: i64,
    #[serde(default)]
    pub secrets: CertSecrets,
}

/// Names of the repo secrets the CI workflow imports the certificates from.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[allow(dead_code)] // read by renew-certs
pub struct CertSecrets {
    pub application: String,
    pub installer: String,
    pub password: String,
}

impl Default for CertSecrets {
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

fn default_main_branch() -> String {
    "main".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionCfg {
    pub preset: Preset,
    /// File holding the version; defaults per preset (Cargo.toml, CMakeLists.txt).
    pub file: Option<PathBuf>,
    /// For `preset: regex`: capture group 1 is the version, used for reading
    /// and for the --next bump.
    pub regex: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    Xcode,
    Cargo,
    Cmake,
    Regex,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ChangelogCfg {
    pub file: PathBuf,
    /// Header suffix of a version still being worked on:
    /// "## [X.Y.Z] - <wip_marker>". A bare "## [X.Y.Z]" counts as open too.
    pub wip_marker: String,
    /// Where --next inserts the new version's section.
    pub insert_next: InsertNext,
    /// Repository URL for keep-a-changelog compare links
    /// ("[Unreleased]: <url>/compare/vX...HEAD"); only with after-unreleased.
    pub compare_links: Option<String>,
}

impl Default for ChangelogCfg {
    fn default() -> Self {
        Self {
            file: "CHANGELOG.md".into(),
            wip_marker: "work in progress".into(),
            insert_next: InsertNext::Top,
            compare_links: None,
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum InsertNext {
    /// Right after the first line (the "# Changelog" title).
    Top,
    /// Right after a "## [Unreleased]" line.
    AfterUnreleased,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NextCfg {
    pub bump: Bump,
    /// Branch name template; `{version}` is replaced.
    pub branch: String,
    /// Shell commands run after the version file is bumped (e.g. `cargo check`
    /// to refresh Cargo.lock).
    pub after_bump: Vec<String>,
    /// Extra files to commit with the bump (e.g. Cargo.lock).
    pub extra_files: Vec<PathBuf>,
}

impl Default for NextCfg {
    fn default() -> Self {
        Self {
            bump: Bump::Minor,
            branch: "dev-{version}".into(),
            after_bump: vec![],
            extra_files: vec![],
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Bump {
    Major,
    Minor,
    Patch,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ReleaseCfg {
    /// Release title template; `{name}` and `{version}` are replaced.
    /// Pre-releases append " (pre-release)".
    pub title: String,
    /// Appended (after a blank line) to every GitHub release's notes, but not
    /// to the squash-merge commit message — it's about how to run the
    /// release, not what changed in it.
    pub notes_suffix: String,
    /// Appended to the squash-merge commit message (e.g. "[skip ci]").
    pub commit_trailer: String,
    /// Extra assets built from the working tree at pre-release time.
    pub extra_assets: Vec<ExtraAsset>,
}

impl Default for ReleaseCfg {
    fn default() -> Self {
        Self {
            title: "{name} {version}".into(),
            notes_suffix: String::new(),
            commit_trailer: String::new(),
            extra_assets: vec![],
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtraAsset {
    /// Directory to zip.
    pub zip: PathBuf,
    /// Asset file name.
    #[serde(rename = "as")]
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum BuildStep {
    /// Local shell commands, then collect the files matching `assets`.
    Commands {
        run: Vec<String>,
        #[serde(default)]
        assets: Vec<String>,
    },
    /// CI workflow triggered by pushing the -pre tag; it publishes the GitHub
    /// pre-release itself. This tool only waits for it.
    GithubTagPush { workflow: String },
    /// CI workflow started via workflow_dispatch against the -pre tag; its
    /// artifacts are downloaded, renamed and uploaded by this tool.
    GithubDispatch {
        workflow: String,
        artifacts: Vec<ArtifactMap>,
    },
    /// Local Xcode archive → Developer ID export → DMG → notarize + staple.
    XcodeDmg(XcodeDmg),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactMap {
    /// Artifact name as uploaded by the workflow.
    pub artifact: String,
    /// Glob, relative to the downloaded artifact directory; must match exactly one file.
    pub file: String,
    /// Asset file name.
    #[serde(rename = "as")]
    pub name: String,
    /// chmod +x before upload (GitHub doesn't preserve it on download anyway,
    /// see release.notes_suffix).
    #[serde(default)]
    pub executable: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XcodeDmg {
    pub project: PathBuf,
    pub scheme: String,
    /// .app name without extension; defaults to the scheme.
    pub app_name: Option<String>,
    pub export_options: PathBuf,
    #[serde(default = "default_build_dir")]
    pub build_dir: PathBuf,
    /// Keychain profile stored with `xcrun notarytool store-credentials`.
    /// The NOTARYTOOL_PROFILE environment variable overrides it.
    #[serde(default = "default_notary_profile")]
    pub notary_profile: String,
}

fn default_build_dir() -> PathBuf {
    "build/release".into()
}

fn default_notary_profile() -> String {
    "notarytool".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppStoreCfg {
    /// Worktree of the orphan branch holding App Store metadata.
    #[serde(default = "default_appstore_worktree")]
    pub worktree: PathBuf,
    pub bundle_id: String,
    /// Xcode project for the iOS build; defaults to the xcode-dmg step's.
    pub project: Option<PathBuf>,
    /// Scheme for the iOS build; defaults to the xcode-dmg step's.
    pub scheme: Option<String>,
    pub export_options: PathBuf,
    #[serde(default = "default_ios_build_dir")]
    pub build_dir: PathBuf,
    #[serde(default = "default_screenshot_max_age")]
    pub screenshot_max_age_days: u64,
    #[serde(default = "default_locale")]
    pub locale: String,
    /// Words that must not appear in the App Store notes (case-insensitive,
    /// whole words), e.g. other platforms a reviewer would object to. A line
    /// that should only go to GitHub carries `<!-- appstore: skip -->`.
    #[serde(default)]
    pub notes_forbidden: Vec<String>,
}

fn default_appstore_worktree() -> PathBuf {
    "appstore".into()
}

fn default_ios_build_dir() -> PathBuf {
    "build/release-ios".into()
}

fn default_screenshot_max_age() -> u64 {
    60
}

fn default_locale() -> String {
    "en-US".into()
}

impl Config {
    pub fn load(root: &Path) -> Result<Config> {
        let path = root.join(FILE_NAME);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Config> {
        let cfg: Config = serde_yaml_ng::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.version.preset == Preset::Regex && self.version.regex.is_none() {
            anyhow::bail!("version.preset: regex needs version.regex");
        }
        if matches!(self.version.preset, Preset::Xcode | Preset::Regex) && self.version.file.is_none()
        {
            anyhow::bail!("version.file is required for this preset");
        }
        if self.changelog.compare_links.is_some()
            && self.changelog.insert_next != InsertNext::AfterUnreleased
        {
            anyhow::bail!("changelog.compare_links needs insert_next: after-unreleased");
        }
        Ok(())
    }

    pub fn title(&self, version: &str) -> String {
        self.release
            .title
            .replace("{name}", &self.name)
            .replace("{version}", version)
    }

    pub fn xcode_dmg(&self) -> Option<&XcodeDmg> {
        self.build.iter().find_map(|s| match s {
            BuildStep::XcodeDmg(x) => Some(x),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_gets_defaults() {
        let cfg = Config::parse("name: X\nversion:\n  preset: cargo\n").unwrap();
        assert_eq!(cfg.main_branch, "main");
        assert_eq!(cfg.changelog.wip_marker, "work in progress");
        assert_eq!(cfg.next.bump, Bump::Minor);
        assert_eq!(cfg.title("1.0.0"), "X 1.0.0");
        assert!(cfg.build.is_empty());
    }

    #[test]
    fn build_steps_parse() {
        let cfg = Config::parse(
            r#"
name: X
version: { preset: xcode, file: X.xcodeproj/project.pbxproj }
build:
  - kind: xcode-dmg
    project: X.xcodeproj
    scheme: X
    export_options: bin/ExportOptions.plist
  - kind: github-dispatch
    workflow: build.yml
    artifacts:
      - { artifact: a, file: "*.AppImage", as: X.AppImage, executable: true }
"#,
        )
        .unwrap();
        assert_eq!(cfg.build.len(), 2);
        assert_eq!(cfg.xcode_dmg().unwrap().notary_profile, "notarytool");
    }

    #[test]
    fn certs_section() {
        let cfg = Config::parse("name: X\nversion: { preset: cargo }\ncerts: { team_id: T }\n").unwrap();
        let certs = cfg.certs.unwrap();
        assert_eq!(certs.min_days, 30);
        assert_eq!(certs.secrets.installer, "APPLE_CERT_INSTALLER_P12_BASE64");
        assert!(Config::parse("name: X\nversion: { preset: cargo }\ncerts: { team_id: T, repos: [] }\n").is_err());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::parse("name: X\nversion: { preset: cargo }\nbogus: 1\n").is_err());
    }

    #[test]
    fn examples_parse() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        let mut n = 0;
        for e in std::fs::read_dir(dir).unwrap() {
            let path = e.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            Config::parse(&text).unwrap_or_else(|err| panic!("{}: {err:#}", path.display()));
            n += 1;
        }
        assert!(n >= 4);
    }

    #[test]
    fn regex_preset_needs_regex() {
        assert!(Config::parse("name: X\nversion: { preset: regex, file: f }\n").is_err());
    }
}
