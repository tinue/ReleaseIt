//! `release --next [--version X.Y.Z]`: from main, cut the next dev branch,
//! bump the version file, open a new "## [X.Y.Z] - <wip>" CHANGELOG section,
//! commit, push. Pure git + file edits, no GitHub calls. With
//! `branching: trunk` no branch is cut: the bump is committed on main or the
//! maintenance branch it runs on.

use crate::changelog;
use crate::config::Branching;
use crate::ctx::Ctx;
use crate::git::{self, StashGuard};
use crate::sh;
use crate::version::{self, VersionFile};
use anyhow::{Result, bail};

pub fn run(ctx: &Ctx, version_override: Option<&str>) -> Result<()> {
    if ctx.cfg.branching == Branching::Trunk {
        return run_trunk(ctx, version_override);
    }
    let main = ctx.cfg.main_branch.as_str();
    if !git::branch_exists_local(main) && !git::branch_exists_remote(main) {
        bail!("'{main}' does not exist yet locally or on origin. Run `release --release` first.");
    }
    let _stash = StashGuard::acquire(ctx)?;
    git::require_clean_index()?;

    println!("Fetching {main}…");
    git::checkout_main_ff(ctx)?;

    let vf = VersionFile::new(&ctx.cfg.version)?;
    // Under --dry-run the checkout above didn't happen, so this reads the
    // starting branch's version — close enough for a preview.
    let current = vf.read(&ctx.root)?;
    let next = match version_override {
        Some(v) => v.to_string(),
        None => version::bump(&current, ctx.cfg.next.bump)?,
    };
    let branch = ctx.cfg.next.branch.replace("{version}", &next);
    if git::branch_exists_local(&branch) {
        bail!("branch '{branch}' already exists locally.");
    }
    if git::branch_exists_remote(&branch) {
        bail!("branch '{branch}' already exists on origin.");
    }

    println!("Starting {next} on branch {branch} (from {main} @ {current})…");
    ctx.run("git", &["checkout", "-b", &branch])?;

    if ctx.dry_run {
        println!("[dry-run] would bump {} from {current} to {next}{}", vf.path.display(), vf.build_reset_note());
    } else {
        vf.write(&ctx.root, &current, &next)?;
    }
    for c in &ctx.cfg.next.after_bump {
        ctx.shell(c)?;
    }
    commit_bump(ctx, &vf, &current, &next, &format!("Bump version to {next}"))?;
    ctx.run("git", &["push", "-u", "origin", &branch])?;

    if ctx.dry_run {
        println!("Done.  [dry-run] No changes made.");
    } else {
        println!("Done.  {branch} is checked out and pushed, starting {next}.");
        println!(
            "  New CHANGELOG entries go under '## [{next}] - {}'; `release` publishes pre-releases and\n  \
             leaves it open, `release --rc` stamps the real date when you're ready.",
            ctx.cfg.changelog.wip_marker
        );
    }
    Ok(())
}

/// Bump the version file, run after_bump, open the CHANGELOG section, commit.
fn commit_bump(ctx: &Ctx, vf: &VersionFile, current: &str, next: &str, msg: &str) -> Result<()> {
    let text = ctx.changelog_text()?;
    let new = changelog::insert_next(&text, &ctx.cfg.changelog, &ctx.cfg.tag_prefix, current, next)?;
    ctx.write_file(&ctx.changelog_path(), &new, &format!("new '## [{next}]' section"))?;

    let mut add = vec!["add", sh::path_str(&vf.path)?, sh::path_str(&ctx.cfg.changelog.file)?];
    for f in &ctx.cfg.next.extra_files {
        add.push(sh::path_str(f)?);
    }
    ctx.run("git", &add)?;
    ctx.run("git", &["commit", "-m", msg])?;
    Ok(())
}

/// Trunk: bump on the current release branch (main or a maintenance branch).
fn run_trunk(ctx: &Ctx, version_override: Option<&str>) -> Result<()> {
    let branch = git::current_branch()?;
    git::require_release_branch(ctx, &branch, "--next")?;
    let _stash = StashGuard::acquire(ctx)?;
    git::require_clean_index()?;
    git::require_in_sync(&branch, &git::upstream(&branch)?)?;

    let vf = VersionFile::new(&ctx.cfg.version)?;
    let current = vf.read(&ctx.root)?;
    if !sh::succeeds("git", &["rev-parse", "-q", "--verify", &format!("refs/tags/{}", ctx.tag)]) {
        bail!("{current} is not released yet (no tag '{}'). Run `release --release` first.", ctx.tag);
    }
    let next = match version_override {
        Some(v) => v.to_string(),
        None => version::bump(&current, ctx.cfg.next.bump)?,
    };
    println!("Starting {next} on {branch} (after {current})…");
    if ctx.dry_run {
        println!("[dry-run] would bump {} from {current} to {next}{}", vf.path.display(), vf.build_reset_note());
    } else {
        vf.write(&ctx.root, &current, &next)?;
    }
    for c in &ctx.cfg.next.after_bump {
        ctx.shell(c)?;
    }
    // Several products share this branch: the message names the product.
    commit_bump(ctx, &vf, &current, &next, &format!("Bump {} version to {next}", ctx.cfg.name))?;
    ctx.run("git", &["push"])?;
    if ctx.dry_run {
        println!("Done.  [dry-run] No changes made.");
    } else {
        println!("Done.  {branch} starts {next}; new CHANGELOG entries go under '## [{next}] - {}'.", ctx.cfg.changelog.wip_marker);
    }
    Ok(())
}
