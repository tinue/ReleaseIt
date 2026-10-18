//! `release ios` — implemented in milestone 4.

use crate::ctx::Ctx;
use anyhow::{Result, bail};

pub fn run(_ctx: &Ctx, _build_only: bool, _release: bool) -> Result<()> {
    bail!("`release ios` is not implemented yet")
}
