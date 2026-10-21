//! Thin helpers around `std::process::Command`.
//!
//! Everything state-changing goes through `Ui::run` (see ui.rs) so that
//! `--dry-run` can print it instead; the helpers here are the raw building
//! blocks and never consult the dry-run flag themselves.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
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

/// Run with `stdin` fed to the child and `env` added; return stdout. On
/// failure the error carries the child's stderr. For secrets, which must
/// not appear on a command line.
pub fn run_with_stdin(prog: &str, args: &[&str], stdin: &[u8], env: &[(&str, &str)]) -> Result<String> {
    let mut child = Command::new(prog)
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start `{prog}`"))?;
    child.stdin.take().context("stdin")?.write_all(stdin)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("`{}` failed ({}): {}", display(prog, args), out.status, String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn require_tool(prog: &str, hint: &str) -> Result<()> {
    if !succeeds("sh", &["-c", &format!("command -v {prog}")]) {
        bail!("'{prog}' not found. {hint}");
    }
    Ok(())
}

/// `cd` to `dir` (the `-C` option), then to the root of its git repository,
/// which both tools treat as the project root; return that root.
pub fn enter_repo_root(dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(dir) = dir {
        std::env::set_current_dir(dir).with_context(|| format!("cannot cd to {}", dir.display()))?;
    }
    let root = PathBuf::from(try_capture("git", &["rev-parse", "--show-toplevel"]).context("not inside a git repository")?);
    std::env::set_current_dir(&root)?;
    Ok(root)
}

/// A path as `&str`, for command-line arguments.
pub fn path_str(p: &Path) -> Result<&str> {
    p.to_str().with_context(|| format!("non-UTF-8 path: {}", p.display()))
}
