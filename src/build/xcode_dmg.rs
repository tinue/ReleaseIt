//! `kind: xcode-dmg` — implemented in milestone 3.

use crate::config::XcodeDmg;
use crate::ctx::Ctx;
use anyhow::{Result, bail};
use std::path::PathBuf;

pub fn build(_ctx: &Ctx, _x: &XcodeDmg, _notarize: bool) -> Result<Vec<PathBuf>> {
    bail!("xcode-dmg is not implemented yet")
}

pub fn signing_identity() -> Result<String> {
    bail!("not implemented yet")
}
