//! Secrets and variables of the current directory's GitHub repo, via the
//! `gh` CLI. Secret values go over stdin, never on the command line.

use anyhow::{Context, Result};
use releaseit::sh;
use releaseit::ui::Ui;
use serde::Deserialize;
use std::collections::HashMap;

pub fn set_secret(ui: &Ui, name: &str, value: &[u8]) -> Result<()> {
    ui.run_with_stdin("gh", &["secret", "set", name], value)
}

pub fn set_variable(ui: &Ui, name: &str, value: &str) -> Result<()> {
    ui.run("gh", &["variable", "set", name, "--body", value])
}

/// All repo variables, in one API call.
pub fn variables() -> Result<HashMap<String, String>> {
    #[derive(Deserialize)]
    struct Var {
        name: String,
        value: String,
    }
    let json = sh::capture("gh", &["variable", "list", "--json", "name,value"])?;
    let vars: Vec<Var> = serde_json::from_str(&json).context("unexpected `gh variable list` output")?;
    Ok(vars.into_iter().map(|v| (v.name, v.value)).collect())
}
