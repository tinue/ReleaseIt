//! GitHub operations via the `gh` CLI (which also handles auth).

use crate::ctx::Ctx;
use crate::sh;
use anyhow::{Result, bail};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub is_prerelease: bool,
    pub is_draft: bool,
    pub assets: Vec<serde_json::Value>,
    pub url: String,
}

pub fn require() -> Result<()> {
    sh::require_tool("gh", "Install with: brew install gh  (then: gh auth login)")
}

/// The release for `tag`, or `None` if there is none.
pub fn view(tag: &str) -> Option<Release> {
    let json = sh::try_capture(
        "gh",
        &["release", "view", tag, "--json", "isPrerelease,isDraft,assets,url"],
    )?;
    serde_json::from_str(&json).ok()
}

/// Create or update the pre-release for `ctx.pre_tag` with `assets`.
pub fn publish_pre(ctx: &Ctx, assets: &[&str]) -> Result<()> {
    let notes = ctx.notes_file()?;
    let notes_path = sh::path_str(notes.path())?;
    let title = format!("{} (pre-release)", ctx.cfg.title(&ctx.version));
    if view(&ctx.pre_tag).is_some() {
        println!("Pre-release {} exists — updating…", ctx.pre_tag);
        ctx.run(
            "gh",
            &["release", "edit", &ctx.pre_tag, "--title", &title, "--notes-file", notes_path, "--prerelease"],
        )?;
        if !assets.is_empty() {
            let mut args = vec!["release", "upload", &ctx.pre_tag];
            args.extend_from_slice(assets);
            args.push("--clobber");
            ctx.run("gh", &args)?;
        }
    } else {
        println!("Creating GitHub pre-release {}…", ctx.pre_tag);
        // --verify-tag: the tag was pushed already; never let gh invent one
        // on the default branch.
        let mut args = vec!["release", "create", &ctx.pre_tag];
        args.extend_from_slice(assets);
        args.extend_from_slice(&["--title", &title, "--notes-file", notes_path, "--prerelease", "--verify-tag"]);
        ctx.run("gh", &args)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Run {
    database_id: u64,
    head_sha: String,
    created_at: String,
}

/// Find the workflow run for `sha` created at or after `since` (RFC 3339,
/// UTC — GitHub's createdAt format, so plain string comparison orders them).
/// Polls for up to a minute, since a run takes a few seconds to appear.
pub fn find_run(workflow: &str, event: &str, sha: &str, since: &str) -> Result<u64> {
    for _ in 0..30 {
        if let Some(json) = sh::try_capture(
            "gh",
            &[
                "run", "list", &format!("--workflow={workflow}"), &format!("--event={event}"),
                &format!("--commit={sha}"), "--json", "databaseId,headSha,createdAt",
            ],
        ) {
            let runs: Vec<Run> = serde_json::from_str(&json).unwrap_or_default();
            if let Some(r) = runs
                .iter()
                .filter(|r| r.head_sha == sha && r.created_at.as_str() >= since)
                .max_by(|a, b| a.created_at.cmp(&b.created_at))
            {
                return Ok(r.database_id);
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    bail!(
        "no {workflow} run for commit {} appeared within 60s.\n  Check manually:  gh run list --workflow={workflow}",
        crate::git::short(sha)
    );
}

pub fn watch(run_id: u64, what: &str) -> Result<()> {
    let id = run_id.to_string();
    // 30 s, not gh's default 3 s: a CI run takes minutes, and a long queue
    // would otherwise spend a large share of the hourly API quota.
    if sh::run("gh", &["run", "watch", &id, "--exit-status", "--interval", "30"]).is_err() {
        bail!(
            "CI run {run_id} for {what} failed.\n  View logs:   gh run view {run_id} --log-failed\n  \
             Fix the problem, then re-run `release` to re-cut the pre-release."
        );
    }
    Ok(())
}

pub fn download(run_id: u64, dir: &Path) -> Result<()> {
    sh::run("gh", &["run", "download", &run_id.to_string(), "-D", sh::path_str(dir)?])
}
