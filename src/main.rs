//! `release` — config-driven release tool. See README.md.

mod appstore;
mod build;
mod changelog;
mod config;
mod ctx;
mod gh;
mod git;
mod modes;
mod sh;
mod version;

use anyhow::{Context, Result};
use clap::{ArgGroup, Parser, Subcommand};
use std::path::PathBuf;

/// Release cycle:
///   release              pre-release "vX.Y.Z-pre" (repeatable; CHANGELOG stays open)
///   release --rc         release candidate: date the CHANGELOG entry, then as above
///   release --release    promote the pre-release to "vX.Y.Z" in place (squash-merge to main)
///   release --next       start the next cycle: cut dev-X.Y.Z from main, bump version
#[derive(Parser)]
#[command(name = "release", disable_version_flag = true, verbatim_doc_comment)]
#[command(group(ArgGroup::new("mode").args(["rc", "release", "next", "build_only"])))]
struct Cli {
    /// Run as if started in this directory.
    #[arg(short = 'C', global = true, value_name = "DIR")]
    dir: Option<PathBuf>,
    /// Print state-changing actions instead of running them.
    #[arg(long, global = true)]
    dry_run: bool,
    /// Answer yes to every confirmation prompt.
    #[arg(long, short = 'y', global = true)]
    yes: bool,

    /// Release candidate: stamp today's date on the CHANGELOG entry, then pre-release.
    #[arg(long)]
    rc: bool,
    /// Promote the pre-release to the real release.
    #[arg(long)]
    release: bool,
    /// Start the next development cycle.
    #[arg(long)]
    next: bool,
    /// Run checks and local build steps only; no git, no GitHub, no notarization.
    #[arg(long)]
    build_only: bool,
    /// With --next: the version to start instead of the configured bump.
    #[arg(long, value_name = "X.Y.Z", requires = "next")]
    version: Option<String>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// iOS/iPadOS: upload a TestFlight build, or (--release) set up the App Store version.
    Ios {
        /// Build the IPA only; no upload.
        #[arg(long, conflicts_with = "release")]
        build_only: bool,
        /// Upload metadata + screenshots and attach the already-uploaded build.
        #[arg(long)]
        release: bool,
    },
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(dir) = &cli.dir {
        std::env::set_current_dir(dir).with_context(|| format!("cannot cd to {}", dir.display()))?;
    }
    let root = PathBuf::from(
        sh::try_capture("git", &["rev-parse", "--show-toplevel"]).context("not inside a git repository")?,
    );
    std::env::set_current_dir(&root)?;
    let cfg = config::Config::load(&root)?;
    let version = version::VersionFile::new(&cfg.version)?.read(&root)?;
    let ctx = ctx::Ctx::new(cfg, root, version, cli.dry_run, cli.yes);

    if let Some(Cmd::Ios { build_only, release }) = cli.cmd {
        let mode_flags = cli.rc || cli.release || cli.next || cli.build_only;
        anyhow::ensure!(!mode_flags, "put ios options after the subcommand: release ios --release");
        return appstore::ios::run(&ctx, build_only, release);
    }
    if cli.next {
        modes::next::run(&ctx, cli.version.as_deref())
    } else if cli.release {
        modes::promote::run(&ctx)
    } else if cli.build_only {
        modes::build_only(&ctx)
    } else {
        modes::pre::run(&ctx, cli.rc)
    }
}
