//! `release --reopen`: undo `--rc`'s date stamp. The dated CHANGELOG header
//! goes back to "## [X.Y.Z] - <wip>" (commit + push), so more entries can
//! follow and `--release` refuses again until the next `--rc`. The -pre tag
//! and GitHub pre-release stay as they are; the next `release` re-cuts them.

use crate::changelog;
use crate::ctx::Ctx;
use crate::git::{self, StashGuard};
use crate::sh;
use anyhow::{Result, bail};

pub fn run(ctx: &Ctx) -> Result<()> {
    let branch = git::current_branch()?;
    git::require_not_main(ctx, &branch, "--reopen")?;
    if sh::succeeds("git", &["rev-parse", "-q", "--verify", &format!("refs/tags/{}", ctx.tag)]) {
        bail!("{} is already released (tag '{}' exists). Nothing to reopen.", ctx.version, ctx.tag);
    }
    let _stash = StashGuard::acquire(ctx)?;
    git::require_clean_index()?;
    git::require_in_sync(&branch, &git::upstream(&branch)?)?;

    let wip = &ctx.cfg.changelog.wip_marker;
    let file = sh::path_str(&ctx.cfg.changelog.file)?;
    let Some(new) = changelog::reopen(&ctx.changelog_text()?, &ctx.version, wip) else {
        println!("Done.  {file}'s {} entry is already open; nothing to do.", ctx.version);
        return Ok(());
    };
    ctx.require_confirm(&format!("Reopen {file}'s {} entry ('- {wip}'), commit and push?", ctx.version))?;
    ctx.write_file(&ctx.changelog_path(), &new, "reopened entry")?;
    ctx.run("git", &["add", file])?;
    ctx.run("git", &["commit", "-m", &format!("Reopen {} CHANGELOG entry", ctx.version)])?;
    ctx.run("git", &["push"])?;
    println!("Done.  {file}'s {} entry is open again; `release --rc` dates it when you're ready.", ctx.version);
    Ok(())
}
