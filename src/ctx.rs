//! Per-invocation context: config, version, tags, and (through `Deref`) the
//! dry-run aware `Ui` every state-changing operation goes through.

use crate::changelog;
use crate::config::Config;
use crate::sh;
use crate::ui::Ui;
use anyhow::{Context, Result};
use std::io::Write;
use std::ops::Deref;
use std::path::PathBuf;

pub struct Ctx {
    pub cfg: Config,
    pub root: PathBuf,
    pub version: String,
    /// The real release tag, "vX.Y.Z". Created exactly once, by --release, on
    /// the squash commit on main.
    pub tag: String,
    /// The disposable pre-release tag, "vX.Y.Z-pre". Pre-releases are cut many
    /// times per version while iterating, so they must not use the real tag:
    /// `gh release create` auto-creates an unknown tag on the current commit,
    /// which is how a real tag once ended up on the previous release's commit
    /// long before that version shipped.
    pub pre_tag: String,
    pub ui: Ui,
}

/// `ctx.run(..)`, `ctx.confirm(..)`, `ctx.dry_run`, … come from the `Ui`.
impl Deref for Ctx {
    type Target = Ui;

    fn deref(&self) -> &Ui {
        &self.ui
    }
}

impl Ctx {
    pub fn new(cfg: Config, root: PathBuf, version: String, ui: Ui) -> Self {
        Self {
            tag: format!("v{version}"),
            pre_tag: format!("v{version}-pre"),
            cfg,
            root,
            version,
            ui,
        }
    }

    /// Run a shell command line from the config (checks, after_bump, …).
    pub fn shell(&self, script: &str) -> Result<()> {
        if self.dry_run {
            println!("[dry-run] {script}");
            Ok(())
        } else {
            println!("$ {script}");
            sh::run("bash", &["-c", script])
        }
    }

    /// Write a file, or describe it under --dry-run.
    pub fn write_file(&self, path: &std::path::Path, content: &str, what: &str) -> Result<()> {
        if self.dry_run {
            println!("[dry-run] would update {} ({what})", path.display());
            Ok(())
        } else {
            std::fs::write(path, content).with_context(|| format!("cannot write {}", path.display()))
        }
    }

    pub fn changelog_path(&self) -> PathBuf {
        self.root.join(&self.cfg.changelog.file)
    }

    pub fn changelog_text(&self) -> Result<String> {
        let p = self.changelog_path();
        std::fs::read_to_string(&p).with_context(|| format!("{} not found", self.cfg.changelog.file.display()))
    }

    /// This version's CHANGELOG section body for GitHub (release notes,
    /// squash-merge message), without `appstore: only` lines. May be empty.
    pub fn notes(&self) -> Result<String> {
        changelog::for_audience(&changelog::notes(&self.changelog_text()?, &self.version), changelog::Audience::GitHub)
    }

    /// The same section for the App Store, without `appstore: skip` lines.
    pub fn appstore_notes(&self) -> Result<String> {
        changelog::for_audience(&changelog::notes(&self.changelog_text()?, &self.version), changelog::Audience::AppStore)
    }

    /// Full GitHub release notes: CHANGELOG section + notes_suffix. Used for
    /// both the pre-release and its promotion, so the suffix is never missing
    /// from one but not the other.
    pub fn release_notes(&self) -> Result<String> {
        let mut notes = self.notes()?;
        if notes.is_empty() {
            notes = format!("Release {}. See {}.", self.tag, self.cfg.changelog.file.display());
        }
        let suffix = self.cfg.release.notes_suffix.trim();
        if !suffix.is_empty() {
            notes.push_str("\n\n");
            notes.push_str(suffix);
        }
        notes.push('\n');
        Ok(notes)
    }

    pub fn notes_file(&self) -> Result<tempfile::NamedTempFile> {
        let mut f = tempfile::NamedTempFile::new()?;
        f.write_all(self.release_notes()?.as_bytes())?;
        Ok(f)
    }
}
