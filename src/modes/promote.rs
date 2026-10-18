//! `release --release`: squash-merge the dev branch onto main as one commit,
//! tag "vX.Y.Z" there, and retarget the "vX.Y.Z-pre" GitHub release onto that
//! tag in place (`gh release edit --tag`) — same release object, same
//! already-uploaded assets, no rebuild, no download/re-upload; the same thing
//! GitHub's own "promote to release" does.
//!
//! Known, accepted quirk: binaries that embed a git-describe build id still
//! show "vX.Y.Z-pre", since promotion preserves the assets.

use crate::changelog;
use crate::ctx::Ctx;
use crate::gh;
use crate::git::{self, StashGuard};
use anyhow::{Context, Result, bail};

pub fn run(ctx: &Ctx) -> Result<()> {
    gh::require()?;
    let branch = git::current_branch()?;
    git::require_not_main(ctx, &branch, "--release")?;
    let _stash = StashGuard::acquire(ctx)?;
    git::require_clean_index()?;

    let text = ctx.changelog_text()?;
    if changelog::is_open(&text, &ctx.version, &ctx.cfg.changelog.wip_marker) {
        bail!(
            "{}'s {} entry is still open (not dated).\n  Run `release --rc` first to close it and build a release candidate.",
            ctx.cfg.changelog.file.display(),
            ctx.version
        );
    }

    let Some(pre) = gh::view(&ctx.pre_tag) else {
        bail!("no GitHub pre-release '{}'. Run `release` first.", ctx.pre_tag);
    };
    if !pre.is_prerelease {
        bail!("'{}' exists but is not a pre-release. Nothing to promote.", ctx.pre_tag);
    }
    if pre.assets.is_empty() {
        bail!("'{}' has no assets attached. Something went wrong building it.", ctx.pre_tag);
    }
    let pre_assets = pre.assets.len();

    ctx.require_confirm(&format!(
        "Promote {} ({pre_assets} assets) to {}: squash-merge '{branch}' into {}, tag, publish?",
        ctx.pre_tag, ctx.tag, ctx.cfg.main_branch
    ))?;

    let notes = ctx.notes()?;
    let mut msg = format!("Release {}", ctx.tag);
    if !notes.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(&notes);
    }
    let trailer = ctx.cfg.release.commit_trailer.trim();
    if !trailer.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(trailer);
    }

    // `git merge --squash` stages only the tracked diff between the branches;
    // unlike `git add -A` it never sweeps up untracked build output.
    let main = ctx.cfg.main_branch.as_str();
    println!("Squash-merging {branch} into {main}…");
    ctx.run("git", &["fetch", "origin", main, "--quiet"])?;
    ctx.run("git", &["checkout", main])?;
    ctx.run("git", &["merge", "--ff-only", &format!("origin/{main}")])?;
    ctx.run("git", &["merge", "--squash", &branch])?;
    ctx.run("git", &["commit", "-m", &msg])?;
    ctx.run("git", &["push", "origin", main])?;

    println!("Tagging {} on {main}…", ctx.tag);
    ctx.run("git", &["tag", &ctx.tag])?;
    ctx.run("git", &["push", "origin", &ctx.tag])?;
    ctx.run("git", &["checkout", &branch])?;

    let notes_file = ctx.notes_file()?;
    let title = ctx.cfg.title(&ctx.version);
    println!("Retargeting {}'s GitHub release onto {} in place (no re-upload)…", ctx.pre_tag, ctx.tag);
    ctx.run(
        "gh",
        &[
            "release", "edit", &ctx.pre_tag, "--tag", &ctx.tag, "--title", &title,
            "--notes-file", notes_file.path().to_str().context("path")?,
            "--prerelease=false", "--latest",
        ],
    )?;

    println!("Removing the now-orphaned {} tag…", ctx.pre_tag);
    ctx.run_lenient("git", &["push", "origin", "--delete", &ctx.pre_tag]);
    ctx.run_lenient("git", &["tag", "-d", &ctx.pre_tag]);

    if ctx.dry_run {
        println!("Done.  [dry-run] No changes made.");
        return Ok(());
    }

    println!("Verifying published release {}…", ctx.tag);
    let r = gh::view(&ctx.tag).with_context(|| format!("release {} not found after promotion", ctx.tag))?;
    if r.assets.len() != pre_assets {
        bail!("release {} has {} asset(s), expected {pre_assets} (unchanged from {})", ctx.tag, r.assets.len(), ctx.pre_tag);
    }
    if r.is_draft {
        bail!("release {} is a draft unexpectedly", ctx.tag);
    }
    if r.is_prerelease {
        bail!("release {} is still marked as a pre-release", ctx.tag);
    }
    println!("Done.  Release {}: {}  ({} assets)", ctx.tag, r.url, r.assets.len());
    Ok(())
}
