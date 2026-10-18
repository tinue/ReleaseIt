//! Git pre-conditions and helpers shared by all modes.

use crate::ctx::Ctx;
use crate::sh;
use anyhow::{Result, bail};

pub fn current_branch() -> Result<String> {
    sh::capture("git", &["rev-parse", "--abbrev-ref", "HEAD"])
}

pub fn head() -> Result<String> {
    sh::capture("git", &["rev-parse", "HEAD"])
}

pub fn short_head() -> Result<String> {
    sh::capture("git", &["rev-parse", "--short", "HEAD"])
}

pub fn require_not_main(ctx: &Ctx, branch: &str, what: &str) -> Result<()> {
    if branch == ctx.cfg.main_branch {
        bail!("{what} must run from a development branch, not '{branch}'.");
    }
    Ok(())
}

/// Unstaged changes are the one pre-condition we recover from instead of
/// aborting: offer to stash them, and pop the stash again when the guard is
/// dropped, however the run ends (success or error). Staged-but-uncommitted
/// changes and unpushed commits still hard-abort — those are work the user
/// must decide about explicitly, not something safe to shelve automatically.
pub struct StashGuard {
    stashed: bool,
}

impl StashGuard {
    pub fn acquire(ctx: &Ctx) -> Result<Self> {
        if sh::succeeds("git", &["diff", "--quiet"]) {
            return Ok(Self { stashed: false });
        }
        println!("Unstaged changes present:");
        sh::run("git", &["status", "--short"])?;
        if !ctx.confirm("Stash them and restore after the release finishes?")? {
            bail!("aborted: commit or stash unstaged changes first.");
        }
        ctx.run("git", &["stash", "push", "-m", "release autostash"])?;
        Ok(Self { stashed: !ctx.dry_run })
    }
}

impl Drop for StashGuard {
    fn drop(&mut self) {
        if self.stashed {
            println!("Restoring stashed changes…");
            if let Err(e) = sh::run("git", &["stash", "pop"]) {
                eprintln!("Warning: {e}. Your changes are still in `git stash list`.");
            }
        }
    }
}

pub fn require_clean_index() -> Result<()> {
    if !sh::succeeds("git", &["diff", "--cached", "--quiet"]) {
        sh::run("git", &["status", "--short"])?;
        bail!("staged but uncommitted changes present. Commit them first.");
    }
    Ok(())
}

pub fn upstream(branch: &str) -> Result<String> {
    match sh::try_capture("git", &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]) {
        // A dev branch created by renaming the previous one keeps tracking
        // the old remote branch; comparing against that would report every
        // commit since as "unpushed".
        Some(u) if !u.is_empty() && u != format!("origin/{branch}") => bail!(
            "branch '{branch}' tracks '{u}' instead of 'origin/{branch}'. Run: git push -u origin {branch}"
        ),
        Some(u) if !u.is_empty() => Ok(u),
        _ => bail!("branch '{branch}' has no upstream. Run: git push -u origin {branch}"),
    }
}

pub fn require_in_sync(branch: &str, upstream: &str) -> Result<()> {
    let local = head()?;
    let Some(remote) = sh::try_capture("git", &["rev-parse", upstream]) else {
        bail!("could not resolve upstream '{upstream}'.");
    };
    if local == remote {
        return Ok(());
    }
    let ahead = sh::capture("git", &["rev-list", "--count", &format!("{upstream}..HEAD")])?;
    if ahead != "0" {
        bail!("{ahead} unpushed commit(s) on '{branch}'. Push before releasing.");
    }
    let behind = sh::capture("git", &["rev-list", "--count", &format!("HEAD..{upstream}")])?;
    bail!("'{branch}' is {behind} commit(s) behind '{upstream}'. Pull before releasing.");
}

/// Fails if main has commits this branch lacks (e.g. a patch merged to main
/// but not back into the dev branch), which would make the squash-merge at
/// --release time conflict or silently revert it.
pub fn require_ff_into_main(ctx: &Ctx, branch: &str) -> Result<()> {
    let main = &ctx.cfg.main_branch;
    sh::run("git", &["fetch", "origin", main, "--quiet"])?;
    let main_sha = sh::capture("git", &["rev-parse", &format!("origin/{main}")])?;
    let base = sh::capture("git", &["merge-base", "HEAD", &format!("origin/{main}")])?;
    if base != main_sha {
        bail!(
            "'{branch}' cannot be fast-forward merged into {main}: {main} has commits not on this branch.\n  \
             Merge or rebase onto {main} first:  git merge origin/{main}"
        );
    }
    Ok(())
}

pub fn branch_exists_local(name: &str) -> bool {
    sh::succeeds("git", &["show-ref", "--verify", "--quiet", &format!("refs/heads/{name}")])
}

pub fn branch_exists_remote(name: &str) -> bool {
    sh::succeeds("git", &["ls-remote", "--exit-code", "--heads", "origin", name])
}
