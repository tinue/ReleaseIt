pub mod next;
pub mod pre;
pub mod promote;
pub mod reopen;

use crate::build::{self, Phase};
use crate::ctx::Ctx;
use anyhow::{Context, Result};

/// Run `release.post_release`. The release is already out at this point, so a
/// failure says that it stands.
fn post_release(ctx: &Ctx) -> Result<()> {
    for c in &ctx.cfg.release.post_release {
        ctx.shell(c)
            .with_context(|| format!("the release is published, but this post_release command failed: {c}"))?;
    }
    Ok(())
}

/// `release --build-only`: checks + local build steps; no git, no GitHub,
/// no notarization.
pub fn build_only(ctx: &Ctx) -> Result<()> {
    println!("{} {}  (build only)", ctx.cfg.name, ctx.version);
    for c in &ctx.cfg.checks {
        ctx.shell(c)?;
    }
    let work = tempfile::tempdir()?;
    let assets = build::run(ctx, Phase::BuildOnly, "", work.path())?;
    if assets.is_empty() {
        println!("Done.  Build-only: checks passed.");
    } else {
        println!("Done.  Build-only:");
        for a in assets {
            println!("  {}", a.display());
        }
    }
    Ok(())
}
