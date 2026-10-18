//! GitHub Actions build steps.
//!
//! `github-tag-push`: the workflow triggers on the -pre tag push and publishes
//! the pre-release itself; we only wait for it.
//!
//! `github-dispatch`: the workflow is workflow_dispatch-only with no publish
//! job; we trigger it against the -pre tag, wait, download its artifacts and
//! hand them back as assets. Each artifact is one complete, directly runnable
//! file (.AppImage, signed/notarized/stapled .dmg, installer .exe) — copied
//! and renamed, never zipped (zipping a .dmg would strip its stapled ticket).

use crate::config::{ArtifactMap, BuildStep};
use crate::ctx::Ctx;
use crate::gh;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

pub struct Pending<'a> {
    workflow: &'a str,
    event: &'static str,
    sha: String,
    since: String,
    artifacts: Option<&'a [ArtifactMap]>,
}

pub fn start<'a>(ctx: &Ctx, step: &'a BuildStep, sha: &str, since: &str) -> Result<Pending<'a>> {
    match step {
        BuildStep::GithubTagPush { workflow } => Ok(Pending {
            workflow,
            event: "push",
            sha: sha.into(),
            since: since.into(),
            artifacts: None,
        }),
        BuildStep::GithubDispatch { workflow, artifacts } => {
            println!("Triggering {workflow} at {} (commit {})…", ctx.pre_tag, &sha[..7]);
            ctx.run("gh", &["workflow", "run", workflow, "--ref", &ctx.pre_tag])?;
            Ok(Pending {
                workflow,
                event: "workflow_dispatch",
                sha: sha.into(),
                since: since.into(),
                artifacts: Some(artifacts),
            })
        }
        _ => unreachable!("not a CI step"),
    }
}

pub fn finish(ctx: &Ctx, p: Pending, work: &Path, assets_dir: &Path) -> Result<Vec<PathBuf>> {
    println!("Waiting for the {} run to appear…", p.workflow);
    let run_id = gh::find_run(p.workflow, p.event, &p.sha, &p.since)?;
    println!("Found run {run_id} — waiting for it to finish…");
    gh::watch(run_id, &ctx.pre_tag)?;

    let Some(artifacts) = p.artifacts else {
        return Ok(vec![]);
    };
    println!("Downloading artifacts from run {run_id}…");
    gh::download(run_id, work)?;
    let mut out = Vec::new();
    for a in artifacts {
        let dir = work.join(&a.artifact);
        if !dir.is_dir() {
            bail!("expected artifact '{}' not found in run {run_id}", a.artifact);
        }
        let pattern = dir.join(&a.file);
        let matches: Vec<PathBuf> = glob::glob(pattern.to_str().context("path")?)?
            .collect::<Result<_, _>>()?;
        let [src] = matches.as_slice() else {
            bail!("artifact '{}': '{}' matched {} files, expected exactly 1", a.artifact, a.file, matches.len());
        };
        let dest = assets_dir.join(&a.name);
        std::fs::copy(src, &dest)?;
        if a.executable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
        }
        println!("  packaged {}", a.name);
        out.push(dest);
    }
    Ok(out)
}
