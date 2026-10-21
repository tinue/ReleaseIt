//! Build orchestration. A project lists one or more build steps; a
//! pre-release run starts every CI step first, runs the local steps while CI
//! builds, then waits for CI and collects everything into one asset list.
//! Nothing is published unless every step succeeds.

mod ci;
mod commands;
pub mod xcode;
mod xcode_dmg;

use crate::config::BuildStep;
use crate::ctx::Ctx;
use crate::sh;
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// --build-only: local steps only, no notarization, no CI.
    BuildOnly,
    /// A real pre-release: everything, including notarization and CI.
    Pre,
}

/// One line per step, for the dry-run preview.
pub fn describe(ctx: &Ctx) -> Vec<String> {
    let mut out: Vec<String> = ctx
        .cfg
        .build
        .iter()
        .map(|s| match s {
            BuildStep::Commands { run, assets } => {
                format!("run {} command(s) locally, collect {}", run.len(), assets.join(", "))
            }
            BuildStep::GithubTagPush { workflow } => {
                format!("wait for {workflow} (triggered by the {} tag push; it publishes the pre-release)", ctx.pre_tag)
            }
            BuildStep::GithubDispatch { workflow, artifacts } => {
                format!("dispatch {workflow} at {}, wait, download {} artifact(s)", ctx.pre_tag, artifacts.len())
            }
            BuildStep::XcodeDmg(x) => {
                format!("archive {} locally → Developer ID export → DMG → notarize + staple", x.scheme)
            }
        })
        .collect();
    for e in &ctx.cfg.release.extra_assets {
        out.push(format!("zip {} as {}", e.zip.display(), e.name));
    }
    out
}

/// Run all build steps. `since` is the UTC time (RFC 3339) just before the
/// -pre tag was pushed, used to tell this run's CI runs from older ones.
pub fn run(ctx: &Ctx, phase: Phase, since: &str, work: &Path) -> Result<Vec<PathBuf>> {
    let steps = &ctx.cfg.build;
    let assets_dir = work.join("assets");
    std::fs::create_dir_all(&assets_dir)?;

    // CI first, so it builds while the local steps run.
    let mut pending = Vec::new();
    let sha = if phase == Phase::Pre { crate::git::head()? } else { String::new() };
    for step in steps.iter().filter(|_| phase == Phase::Pre) {
        match step {
            BuildStep::GithubTagPush { workflow } => pending.push(ci::tag_push(workflow)),
            BuildStep::GithubDispatch { workflow, artifacts } => {
                pending.push(ci::dispatch(ctx, workflow, artifacts, &sha)?)
            }
            BuildStep::Commands { .. } | BuildStep::XcodeDmg(_) => {}
        }
    }

    let mut assets = Vec::new();
    for step in steps {
        let result = match step {
            BuildStep::Commands { run, assets } => commands::run(ctx, run, assets),
            BuildStep::XcodeDmg(x) => xcode_dmg::build(ctx, x, phase == Phase::Pre),
            BuildStep::GithubTagPush { .. } | BuildStep::GithubDispatch { .. } => continue,
        };
        match result {
            Ok(a) => assets.extend(a),
            Err(e) if !pending.is_empty() => {
                return Err(e.context("local build failed (CI runs already started keep running; nothing was published)"));
            }
            Err(e) => return Err(e),
        }
    }

    for (i, p) in pending.into_iter().enumerate() {
        assets.extend(ci::finish(ctx, p, &sha, since, &work.join(format!("ci-{i}")), &assets_dir)?);
    }

    if phase == Phase::Pre {
        for e in &ctx.cfg.release.extra_assets {
            let out = assets_dir.join(&e.name);
            println!("Zipping {}…", e.zip.display());
            sh::run("zip", &["-rq", sh::path_str(&out)?, sh::path_str(&e.zip)?])?;
            assets.push(out);
        }
    }

    let mut names = HashSet::new();
    for a in &assets {
        let name = a.file_name().context("asset without file name")?.to_string_lossy().into_owned();
        if !names.insert(name.clone()) {
            bail!("two build steps produced an asset named '{name}'");
        }
    }
    Ok(assets)
}
