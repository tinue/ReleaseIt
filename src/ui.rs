//! Dry-run aware command runner and prompts, shared by `release` (through
//! `Ctx`) and `renew-certs`. Every state-changing command goes through here so
//! that `--dry-run` prints it instead.

use crate::sh;
use anyhow::{Result, bail};
use std::io::{BufRead, Write};

pub struct Ui {
    pub dry_run: bool,
    pub yes: bool,
}

impl Ui {
    /// Run a state-changing command, or print it under --dry-run.
    pub fn run(&self, prog: &str, args: &[&str]) -> Result<()> {
        if self.dry_run {
            println!("[dry-run] {}", sh::display(prog, args));
            Ok(())
        } else {
            sh::run(prog, args)
        }
    }

    /// Like `run`, but failure is only reported, not fatal (cleanup steps).
    pub fn run_lenient(&self, prog: &str, args: &[&str]) {
        if let Err(e) = self.run(prog, args) {
            println!("  (ignored: {e})");
        }
    }

    /// Like `run`, with `stdin` fed to the command (secrets). The dry-run
    /// output shows only its size.
    pub fn run_with_stdin(&self, prog: &str, args: &[&str], stdin: &[u8]) -> Result<()> {
        if self.dry_run {
            println!("[dry-run] {}  ({} bytes on stdin)", sh::display(prog, args), stdin.len());
            Ok(())
        } else {
            sh::run_with_stdin(prog, args, stdin, &[]).map(drop)
        }
    }

    /// A state-changing step that isn't a command line: `f` runs, or `what`
    /// is printed under --dry-run.
    pub fn act(&self, what: &str, f: impl FnOnce() -> Result<()>) -> Result<()> {
        if self.dry_run {
            println!("[dry-run] {what}");
            Ok(())
        } else {
            f()
        }
    }

    /// Ask a yes/no question. --yes answers yes; --dry-run answers yes too so a
    /// preview runs through without interaction.
    pub fn confirm(&self, prompt: &str) -> Result<bool> {
        if self.yes || self.dry_run {
            println!("{prompt} [y/N] yes{}", if self.dry_run && !self.yes { "  [dry-run]" } else { "" });
            return Ok(true);
        }
        print!("{prompt} [y/N] ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
    }

    pub fn require_confirm(&self, prompt: &str) -> Result<()> {
        if !self.confirm(prompt)? {
            bail!("aborted");
        }
        Ok(())
    }
}

pub fn section(title: &str) {
    let pad = 64usize.saturating_sub(title.chars().count() + 4);
    println!("\n── {title} {}", "─".repeat(pad));
}

pub fn end_section() {
    println!("{}\n", "─".repeat(64));
}

/// `body` between a section header and its closing rule.
pub fn boxed(title: &str, body: &str) {
    section(title);
    println!("{}", body.trim_end());
    end_section();
}
