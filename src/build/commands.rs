//! `kind: commands` — local shell commands, then collect asset globs.

use crate::ctx::Ctx;
use crate::sh;
use anyhow::{Result, bail};
use std::path::PathBuf;

pub fn run(ctx: &Ctx, commands: &[String], globs: &[String]) -> Result<Vec<PathBuf>> {
    for c in commands {
        ctx.shell(c)?;
    }
    let mut assets = Vec::new();
    for g in globs {
        let pattern = ctx.root.join(g);
        let matches: Vec<PathBuf> = glob::glob(sh::path_str(&pattern)?)?
            .collect::<Result<_, _>>()?;
        if matches.is_empty() {
            if ctx.dry_run {
                println!("[dry-run] would collect {g}");
                continue;
            }
            bail!("build produced no file matching '{g}'");
        }
        assets.extend(matches);
    }
    Ok(assets)
}
