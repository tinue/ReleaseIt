//! Xcode helpers shared by the macOS DMG step and the iOS flow: xcodebuild
//! with filtered output, Developer ID identity lookup, notarization.

use crate::sh;
use anyhow::{Context, Result, bail};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

pub fn require_xcode() -> Result<()> {
    let selected = sh::try_capture("xcode-select", &["-p"]).unwrap_or_default();
    if !selected.contains("Xcode.app") {
        bail!("Xcode not selected. Run:\n  sudo xcode-select -s /Applications/Xcode.app/Contents/Developer");
    }
    Ok(())
}

/// Run xcodebuild, echoing only lines that start with one of `prefixes`
/// (plus errors and warnings), minus the noisy provisioning-profile chatter.
/// On failure, the last unfiltered lines are printed for diagnosis.
pub fn xcodebuild(args: &[&str], prefixes: &[&str]) -> Result<()> {
    let mut child = Command::new("xcodebuild")
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to start xcodebuild")?;
    let mut tail: VecDeque<String> = VecDeque::with_capacity(40);
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let line = line?;
        let shown = (prefixes.iter().any(|p| line.starts_with(p))
            || line.starts_with("error:")
            || line.starts_with("warning:"))
            && !line.contains("provisioning profile");
        if shown {
            println!("{line}");
        }
        if tail.len() == 40 {
            tail.pop_front();
        }
        tail.push_back(line);
    }
    let status = child.wait()?;
    if !status.success() {
        println!("── last xcodebuild output ──");
        for l in tail {
            println!("{l}");
        }
        bail!("xcodebuild {} failed ({status})", args.first().unwrap_or(&""));
    }
    Ok(())
}

/// The Developer ID Application identity: $SIGN_IDENTITY, or the first one
/// in the keychain.
pub fn signing_identity() -> Result<String> {
    if let Ok(id) = std::env::var("SIGN_IDENTITY")
        && !id.is_empty()
    {
        return Ok(id);
    }
    let out = sh::try_capture("security", &["find-identity", "-v", "-p", "codesigning"]).unwrap_or_default();
    out.lines()
        .find_map(|l| {
            let start = l.find("\"Developer ID Application:")? + 1;
            let end = l[start..].find('"')? + start;
            Some(l[start..end].to_string())
        })
        .context(
            "no 'Developer ID Application' signing identity found in keychain.\n  \
             Install one in Xcode (Settings → Accounts → Manage Certificates)\n  \
             or set SIGN_IDENTITY explicitly.",
        )
}

/// Submit to the notary service, wait, fetch the log on rejection, staple.
pub fn notarize(profile: &str, file: &Path) -> Result<()> {
    let profile = std::env::var("NOTARYTOOL_PROFILE").unwrap_or_else(|_| profile.to_string());
    if !sh::succeeds("xcrun", &["notarytool", "history", "--keychain-profile", &profile]) {
        bail!(
            "notarytool profile '{profile}' not found. Store credentials once with:\n  \
             xcrun notarytool store-credentials \"{profile}\" \\\n    \
             --apple-id \"your@apple.id\" --team-id <TEAMID> \\\n    \
             --password \"<app-specific-password>\"   # appleid.apple.com → App-Specific Passwords"
        );
    }
    let path = file.to_str().context("path")?;
    println!("Notarizing (this may take a minute)…");
    let (_, out) = sh::capture_all(
        "xcrun",
        &["notarytool", "submit", path, "--keychain-profile", &profile, "--wait"],
    )?;
    println!("{out}");
    let (id, status) = parse_notary_output(&out);
    if status.as_deref() != Some("Accepted") {
        println!("Notarization failed (status: {}).", status.as_deref().unwrap_or("unknown"));
        if let Some(id) = id {
            println!("Fetching rejection log for {id}…");
            let _ = sh::run("xcrun", &["notarytool", "log", &id, "--keychain-profile", &profile]);
        }
        bail!("notarization of {} failed", file.display());
    }
    sh::run("xcrun", &["stapler", "staple", path])?;
    println!("Notarized and stapled.");
    Ok(())
}

/// First "id:" line and last "status:" line of `notarytool submit --wait`.
fn parse_notary_output(out: &str) -> (Option<String>, Option<String>) {
    let field = |l: &str, key: &str| l.trim_start().strip_prefix(key).map(|v| v.trim().to_string());
    let id = out.lines().find_map(|l| field(l, "id:"));
    let status = out.lines().rev().find_map(|l| field(l, "status:"));
    (id, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_notary_output() {
        let out = "Conducting pre-submission checks…\nSubmission ID received\n  id: 1234-abcd\nUpload progress: 100%\n\
                   Waiting for processing to complete.\nCurrent status: In Progress....\nProcessing complete\n  id: 1234-abcd\n  status: Accepted\n";
        assert_eq!(parse_notary_output(out), (Some("1234-abcd".into()), Some("Accepted".into())));
        assert_eq!(parse_notary_output("nothing"), (None, None));
    }
}
