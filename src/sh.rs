//! Thin helpers around `std::process::Command`.
//!
//! Everything state-changing goes through `Ctx::run` (see ctx.rs) so that
//! `--dry-run` can print it instead; the helpers here are the raw building
//! blocks and never consult the dry-run flag themselves.

use anyhow::{Context, Result, bail};
use std::process::{Command, Stdio};

/// Render a command line for display (dry-run output, error messages).
pub fn display(prog: &str, args: &[&str]) -> String {
    let mut out = String::from(prog);
    for a in args {
        out.push(' ');
        if a.is_empty() || a.contains(|c: char| c.is_whitespace() || "\"'$`\\".contains(c)) {
            out.push('\'');
            out.push_str(&a.replace('\'', r"'\''"));
            out.push('\'');
        } else {
            out.push_str(a);
        }
    }
    out
}

/// Run with inherited stdio; error if the command fails.
pub fn run(prog: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(prog)
        .args(args)
        .status()
        .with_context(|| format!("failed to start `{prog}`"))?;
    if !status.success() {
        bail!("`{}` failed ({status})", display(prog, args));
    }
    Ok(())
}

/// Run and capture stdout (trimmed); stderr is inherited. Error if it fails.
pub fn capture(prog: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(prog)
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("failed to start `{prog}`"))?;
    if !out.status.success() {
        bail!("`{}` failed ({})", display(prog, args), out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Run silently; return whether it succeeded. For probes like
/// `git diff --quiet` or `gh release view`.
pub fn succeeds(prog: &str, args: &[&str]) -> bool {
    Command::new(prog)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run silently and capture stdout if it succeeds; `None` otherwise.
pub fn try_capture(prog: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(prog)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Run and capture stdout+stderr combined, regardless of exit status.
pub fn capture_all(prog: &str, args: &[&str]) -> Result<(bool, String)> {
    let out = Command::new(prog)
        .args(args)
        .output()
        .with_context(|| format!("failed to start `{prog}`"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), text))
}

/// Is `prog` on PATH?
pub fn have(prog: &str) -> bool {
    succeeds("sh", &["-c", &format!("command -v {prog}")])
}

pub fn require_tool(prog: &str, hint: &str) -> Result<()> {
    if !have(prog) {
        bail!("'{prog}' not found. {hint}");
    }
    Ok(())
}
