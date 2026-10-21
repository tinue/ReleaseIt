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

use crate::config::ArtifactMap;
use crate::ctx::Ctx;
use crate::{gh, git, sh};
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

pub struct Pending<'a> {
    workflow: &'a str,
    event: &'static str,
    artifacts: Option<&'a [ArtifactMap]>,
}

/// `github-tag-push`: the tag push already started it.
pub fn tag_push(workflow: &str) -> Pending<'_> {
    Pending { workflow, event: "push", artifacts: None }
}

/// `github-dispatch`: start it against the -pre tag.
pub fn dispatch<'a>(ctx: &Ctx, workflow: &'a str, artifacts: &'a [ArtifactMap], sha: &str) -> Result<Pending<'a>> {
    println!("Triggering {workflow} at {} (commit {})…", ctx.pre_tag, git::short(sha));
    ctx.run("gh", &["workflow", "run", workflow, "--ref", &ctx.pre_tag])?;
    Ok(Pending { workflow, event: "workflow_dispatch", artifacts: Some(artifacts) })
}

/// Wait for the run of `p` for commit `sha`, started at or after `since`,
/// and collect its artifacts.
pub fn finish(ctx: &Ctx, p: Pending, sha: &str, since: &str, work: &Path, assets_dir: &Path) -> Result<Vec<PathBuf>> {
    println!("Waiting for the {} run to appear…", p.workflow);
    let run_id = gh::find_run(p.workflow, p.event, sha, since)?;
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
        let matches: Vec<PathBuf> = glob::glob(sh::path_str(&pattern)?)?
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
