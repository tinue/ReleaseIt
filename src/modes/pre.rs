//! `release` / `release --rc`: build per config and publish (or update) the
//! GitHub pre-release under the disposable "vX.Y.Z-pre" tag. Re-cut as often
//! as you like while iterating.
//!
//! Plain `release` never touches the CHANGELOG, so more entries can follow.
//! `--rc` first stamps today's date on the version's open CHANGELOG header
//! (commit + push) — this happens once; a real date is never overwritten.

use crate::build::{self, Phase};
use crate::changelog;
use crate::config::BuildStep;
use crate::ctx::Ctx;
use crate::git::{self, StashGuard};
use crate::ui;
use crate::{appstore, gh, sh};
use anyhow::{Context, Result, bail};

pub fn run(ctx: &Ctx, rc: bool) -> Result<()> {
    gh::require()?;
    let branch = git::current_branch()?;
    git::require_not_main(ctx, &branch, "A pre-release")?;
    let _stash = StashGuard::acquire(ctx)?;
    git::require_clean_index()?;
    let upstream = git::upstream(&branch)?;

    // Checked before the --rc date stamp (which commits + pushes) so an empty
    // or missing section aborts before anything is mutated or pushed.
    let notes = ctx.notes()?;
    if notes.is_empty() {
        bail!(
            "no '## [{}]' entry (or an empty one) in {}. Add one before releasing.",
            ctx.version,
            ctx.cfg.changelog.file.display()
        );
    }
    if rc {
        stamp_changelog_date(ctx)?;
    }
    git::require_in_sync(&branch, &upstream)?;
    git::require_ff_into_main(ctx, &branch)?;

    println!("{} {}  (pre-release tag: {})", ctx.cfg.name, ctx.version, ctx.pre_tag);
    for c in &ctx.cfg.checks {
        ctx.shell(c)?;
    }

    // GitHub only dispatches a workflow whose file is on the default branch
    // (`gh workflow run` gets a 404 otherwise). Catch that before the tag push.
    for step in &ctx.cfg.build {
        if let BuildStep::GithubDispatch { workflow, .. } = step
            && !sh::succeeds("gh", &["workflow", "view", workflow])
        {
            bail!(
                "GitHub can't dispatch {workflow}: it isn't on the default branch '{}' yet.\n  \
                 GitHub only runs workflow_dispatch workflows whose file exists there; commit it to {} first.",
                ctx.cfg.main_branch,
                ctx.cfg.main_branch
            );
        }
    }

    if let Some(app) = &ctx.cfg.appstore {
        appstore::whatsnew::publish(ctx, app, &ctx.appstore_notes()?)?;
    }

    ui::boxed(
        &format!("Release notes for {} (from {})", ctx.pre_tag, ctx.cfg.changelog.file.display()),
        &ctx.release_notes()?,
    );
    println!("Build plan:");
    for line in build::describe(ctx) {
        println!("  • {line}");
    }
    let kind = if rc { "release candidate" } else { "pre-release (CHANGELOG entry stays open)" };
    ctx.require_confirm(&format!(
        "Push tag '{}' at {}, build, and publish a {kind}?",
        ctx.pre_tag,
        git::short_head()?
    ))?;

    let since = (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    // -f: the -pre tag is disposable and moves to the latest pre-release commit.
    ctx.run("git", &["tag", "-f", &ctx.pre_tag])?;
    ctx.run("git", &["push", "-f", "origin", &ctx.pre_tag])?;
    if ctx.dry_run {
        println!("Done.  [dry-run] No build, nothing published.");
        return Ok(());
    }

    let work = tempfile::tempdir()?;
    let assets = build::run(ctx, Phase::Pre, &since, work.path())?;

    // With no assets of its own, a github-tag-push workflow published it.
    let ci_publishes = ctx.cfg.build.iter().any(|s| matches!(s, BuildStep::GithubTagPush { .. }));
    if !(assets.is_empty() && ci_publishes) {
        let paths: Vec<&str> = assets.iter().map(|p| sh::path_str(p)).collect::<Result<_>>()?;
        gh::publish_pre(ctx, &paths)?;
    }
    let r = gh::view(&ctx.pre_tag)
        .with_context(|| format!("the build finished but there is no GitHub release '{}'", ctx.pre_tag))?;
    println!("Done.  Pre-release {}: {}  ({} assets)", ctx.pre_tag, r.url, r.assets.len());
    Ok(())
}

fn stamp_changelog_date(ctx: &Ctx) -> Result<()> {
    let text = ctx.changelog_text()?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let Some(new) = changelog::stamp_date(&text, &ctx.version, &ctx.cfg.changelog.wip_marker, &today) else {
        return Ok(()); // already dated
    };
    let file = sh::path_str(&ctx.cfg.changelog.file)?;
    println!("Setting {file} release date for {} to {today}…", ctx.version);
    ctx.write_file(&ctx.changelog_path(), &new, "release date")?;
    ctx.run("git", &["add", file])?;
    ctx.run("git", &["commit", "-m", &format!("Set {} release date to {today}", ctx.version)])?;
    ctx.run("git", &["push"])?;
    Ok(())
}
