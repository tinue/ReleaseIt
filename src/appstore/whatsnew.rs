//! App Store "What's New" text: the CHANGELOG section converted from
//! markdown to the plain text App Store Connect accepts, written to
//! `<worktree>/whatsnew.txt` and committed on the appstore branch during a
//! pre-release, so `release ios --release` uploads exactly what was reviewed.

use crate::config::AppStoreCfg;
use crate::ctx::{self, Ctx};
use crate::sh;
use anyhow::{Context, Result, bail};
use regex::Regex;

/// Markdown → App Store plain text:
/// "### Fixes" → blank line + "FIXES"; "- **Cat** text" → "• CAT: text";
/// "**Bold line**" → "• Bold line"; other `**` removed; `code` → "code";
/// < and > → ‹ and › (App Store Connect rejects angle brackets).
pub fn convert(notes: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in notes.lines().filter(|l| !l.is_empty()) {
        if let Some(rest) = line.strip_prefix("### ") {
            out.push(String::new());
            out.push(rest.trim_start_matches(' ').to_ascii_uppercase());
            continue;
        }
        let mut line = line.to_string();
        if let Some(rest) = line.strip_prefix("- **") {
            if let Some(n) = rest.find("**") {
                let cat = &rest[..n];
                let tail = &rest[n + 2..];
                let tail = tail.strip_prefix(' ').unwrap_or(tail);
                out.push(format!("• {}: {tail}", cat.to_ascii_uppercase()));
                continue;
            }
            // No closing "**": fall through with the prefix stripped.
            line = rest.to_string();
        }
        if line.starts_with("**") {
            let l = line.replace("**", "");
            if !l.is_empty() {
                out.push(format!("• {l}"));
            }
            continue;
        }
        out.push(line.replace("**", ""));
    }
    let text = out.join("\n");
    let text = backticks_to_quotes(&text).replace('<', "‹").replace('>', "›");
    text.trim_end_matches('\n').to_string()
}

fn backticks_to_quotes(s: &str) -> String {
    // Per line, pair up backticks like sed 's/`\([^`]*\)`/"\1"/g'.
    s.lines()
        .map(|l| {
            let mut out = String::new();
            let mut rest = l;
            while let Some(a) = rest.find('`') {
                let after = &rest[a + 1..];
                let Some(b) = after.find('`') else { break };
                out.push_str(&rest[..a]);
                out.push('"');
                out.push_str(&after[..b]);
                out.push('"');
                rest = &after[b + 1..];
            }
            out.push_str(rest);
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The lines of `text` that contain one of `words` (case-insensitive, whole
/// words).
pub fn forbidden_lines<'a>(text: &'a str, words: &[String]) -> Vec<&'a str> {
    if words.is_empty() {
        return vec![];
    }
    let alternatives: Vec<String> = words.iter().map(|w| regex::escape(w)).collect();
    let re = Regex::new(&format!(r"(?i)\b({})\b", alternatives.join("|"))).unwrap();
    text.lines().filter(|l| re.is_match(l)).collect()
}

pub fn check_forbidden(text: &str, words: &[String]) -> Result<()> {
    let hits = forbidden_lines(text, words);
    if !hits.is_empty() {
        bail!(
            "the App Store release notes mention {} (appstore.notes_forbidden):\n  {}\n  \
             Mark those CHANGELOG lines with <!-- appstore: skip --> (or reword them).",
            words.join(", "),
            hits.join("\n  ")
        );
    }
    Ok(())
}

/// `notes`: the CHANGELOG section already filtered for the App Store
/// (`Ctx::appstore_notes`).
pub fn publish(ctx: &Ctx, app: &AppStoreCfg, notes: &str) -> Result<()> {
    let text = convert(notes);
    if ctx.dry_run {
        ctx::section("Draft App Store release notes (whatsnew.txt)");
        println!("{text}");
        ctx::end_section();
        if let Err(e) = check_forbidden(&text, &app.notes_forbidden) {
            println!("Warning: {e:#}");
        }
        return Ok(());
    }
    if text.is_empty() {
        bail!("every line of the CHANGELOG entry is marked <!-- appstore: skip --> — nothing left for the App Store.");
    }
    check_forbidden(&text, &app.notes_forbidden)?;
    ctx::section("App Store release notes (whatsnew.txt)");
    println!("{text}");
    ctx::end_section();
    ctx.require_confirm("Use these release notes for App Store?")?;

    let wt = ctx.root.join(&app.worktree);
    if !wt.join(".git").is_file() {
        println!("Warning: {}/ worktree not found — skipping whatsnew.txt.", app.worktree.display());
        println!("  Set it up once with: git worktree add {0} {0}", app.worktree.display());
        return Ok(());
    }
    std::fs::write(wt.join("whatsnew.txt"), format!("{text}\n"))?;
    let wt_s = wt.to_str().context("path")?;
    sh::run("git", &["-C", wt_s, "add", "-f", "whatsnew.txt"])?;
    if sh::succeeds("git", &["-C", wt_s, "diff", "--cached", "--quiet"]) {
        println!("  (whatsnew.txt unchanged — no commit needed)");
    } else {
        sh::run("git", &["-C", wt_s, "commit", "-m", &format!("whatsnew: v{}", ctx.version)])?;
        sh::run("git", &["-C", wt_s, "push"])?;
        println!("Release notes committed and pushed to the {} branch.", app.worktree.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_forbidden_words() {
        let words = vec!["Windows".to_string(), "Linux".to_string()];
        let text = "• DESKTOP: New app for windows and Linux\n• DISPLAY: Better\n• WINDOWSILL: not a whole word";
        assert_eq!(forbidden_lines(text, &words), vec!["• DESKTOP: New app for windows and Linux"]);
        assert!(forbidden_lines(text, &[]).is_empty());
        assert!(check_forbidden("• DISPLAY: Better", &words).is_ok());
    }

    #[test]
    fn converts() {
        let md = "- **Keyboard** Fixed `x` <y>\n\n### Fixes\n- plain **bold** item\n**Whole**\n- **Unclosed";
        assert_eq!(
            convert(md),
            "• KEYBOARD: Fixed \"x\" ‹y›\n\nFIXES\n- plain bold item\n• Whole\nUnclosed"
        );
    }

    /// Golden output produced by the original bin/release awk/sed pipeline on
    /// Calc-U-59's 1.7.0 notes (tests/fixtures/whatsnew-1.7.0.txt).
    #[test]
    fn matches_original_pipeline() {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let changelog = std::fs::read_to_string(dir.join("CHANGELOG-Calc-U-59.md")).unwrap();
        let expected = std::fs::read_to_string(dir.join("whatsnew-1.7.0.txt")).unwrap();
        let notes = crate::changelog::notes(&changelog, "1.7.0");
        assert_eq!(format!("{}\n", convert(&notes)), expected);
    }
}
