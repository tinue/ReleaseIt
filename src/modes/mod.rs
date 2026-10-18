pub mod next;
pub mod pre;
pub mod promote;

use crate::build::{self, Phase};
use crate::ctx::Ctx;
use anyhow::Result;

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
