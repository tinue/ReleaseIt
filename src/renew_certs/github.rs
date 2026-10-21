//! Repo secrets and variables via the `gh` CLI. Secret values go over
//! stdin, never on the command line.

use crate::sh;
use anyhow::{Context, Result, bail};
use std::io::Write;
use std::process::{Command, Stdio};

pub fn require() -> Result<()> {
    sh::require_tool("gh", "Install with: brew install gh  (then: gh auth login)")
}

pub fn set_secret(repo: &str, name: &str, value: &[u8]) -> Result<()> {
    let args = ["secret", "set", name, "-R", repo];
    let mut child = Command::new("gh")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("failed to start `gh`")?;
    child.stdin.take().context("stdin")?.write_all(value)?;
    let status = child.wait()?;
    if !status.success() {
        bail!("`{}` failed ({status})", sh::display("gh", &args));
    }
    Ok(())
}

/// A repo variable's value, `None` if it isn't set.
pub fn variable(repo: &str, name: &str) -> Option<String> {
    sh::try_capture("gh", &["variable", "get", name, "-R", repo]).filter(|v| !v.is_empty())
}

pub fn set_variable_args<'a>(repo: &'a str, name: &'a str, value: &'a str) -> [&'a str; 7] {
    ["variable", "set", name, "-R", repo, "--body", value]
}
