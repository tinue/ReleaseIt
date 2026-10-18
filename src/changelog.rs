//! CHANGELOG.md handling: one "## [X.Y.Z] - <date or wip marker>" section per
//! version, newest first.

use crate::config::{ChangelogCfg, InsertNext};
use anyhow::{Result, bail};
use regex::Regex;

fn header_re(version: &str) -> Regex {
    Regex::new(&format!(r"^## \[{}\](\s|$)", regex::escape(version))).unwrap()
}

fn open_header_re(version: &str, wip_marker: &str) -> Regex {
    Regex::new(&format!(
        r"^## \[{}\]( - {})?$",
        regex::escape(version),
        regex::escape(wip_marker)
    ))
    .unwrap()
}

/// The section body for `version`, with leading blank lines and trailing
/// blank lines trimmed. Empty if there is no such section (or it is empty).
/// The section ends at the next "## " header or at a keep-a-changelog link
/// reference ("[1.0.0]: https://…").
pub fn notes(text: &str, version: &str) -> String {
    let header = header_re(version);
    let link_ref = Regex::new(r"^\[[^\]\[]+\]: ").unwrap();
    let mut lines = text.lines().skip_while(|l| !header.is_match(l));
    if lines.next().is_none() {
        return String::new();
    }
    let body: Vec<&str> = lines
        .take_while(|l| !l.starts_with("## ") && !link_ref.is_match(l))
        .skip_while(|l| l.trim().is_empty())
        .collect();
    let mut out = body.join("\n");
    out.truncate(out.trim_end().len());
    out
}

/// True while the version's header is still open: "## [X] - <wip>" or a bare
/// "## [X]" without a date.
pub fn is_open(text: &str, version: &str, wip_marker: &str) -> bool {
    let re = open_header_re(version, wip_marker);
    text.lines().any(|l| re.is_match(l))
}

/// Replace an open header with "## [X] - <date>". `None` if the header
/// already carries a date (never overwritten).
pub fn stamp_date(text: &str, version: &str, wip_marker: &str, date: &str) -> Option<String> {
    let re = open_header_re(version, wip_marker);
    let mut changed = false;
    let out = map_lines(text, |l| {
        if !changed && re.is_match(l) {
            changed = true;
            vec![format!("## [{version}] - {date}")]
        } else {
            vec![l.to_string()]
        }
    });
    changed.then_some(out)
}

/// Insert an empty "## [next] - <wip>" section, plus compare links if
/// configured. Errors if the insertion point isn't there.
pub fn insert_next(text: &str, cfg: &ChangelogCfg, current: &str, next: &str) -> Result<String> {
    let header = format!("## [{next}] - {}", cfg.wip_marker);
    let mut inserted = false;
    let mut out = match cfg.insert_next {
        InsertNext::Top => map_lines(text, |l| {
            if !inserted {
                inserted = true;
                vec![l.to_string(), String::new(), header.clone()]
            } else {
                vec![l.to_string()]
            }
        }),
        InsertNext::AfterUnreleased => map_lines(text, |l| {
            if !inserted && l == "## [Unreleased]" {
                inserted = true;
                vec![l.to_string(), String::new(), header.clone()]
            } else {
                vec![l.to_string()]
            }
        }),
    };
    if !inserted {
        bail!(
            "could not insert '{header}' into {}: {}",
            cfg.file.display(),
            match cfg.insert_next {
                InsertNext::Top => "the file is empty",
                InsertNext::AfterUnreleased => "no '## [Unreleased]' line",
            }
        );
    }
    if let Some(repo) = &cfg.compare_links {
        let repo = repo.trim_end_matches('/');
        let mut found = false;
        out = map_lines(&out, |l| {
            if !found && l.starts_with("[Unreleased]: ") {
                found = true;
                vec![
                    format!("[Unreleased]: {repo}/compare/v{next}...HEAD"),
                    format!("[{next}]: {repo}/compare/v{current}...v{next}"),
                ]
            } else {
                vec![l.to_string()]
            }
        });
        if !found {
            bail!("no '[Unreleased]: ' compare link in {}", cfg.file.display());
        }
    }
    Ok(out)
}

/// Line-wise rewrite that keeps the file's trailing-newline state.
fn map_lines(text: &str, mut f: impl FnMut(&str) -> Vec<String>) -> String {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines() {
        out.extend(f(l));
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        std::fs::read_to_string(p).unwrap()
    }

    const SIMPLE: &str = "# Changelog\n\n## [1.8.0] - work in progress\n\n- **A** one\n\n### Fixes\n- two\n\n## [1.7.0] - 2026-09-15\n- old\n";

    #[test]
    fn extracts_notes() {
        assert_eq!(notes(SIMPLE, "1.8.0"), "- **A** one\n\n### Fixes\n- two");
        assert_eq!(notes(SIMPLE, "1.7.0"), "- old");
        assert_eq!(notes(SIMPLE, "1.6.0"), "");
        // "1.8.0" must not match "1.8.01" or "1x8x0"
        assert_eq!(notes("## [1.8.01]\n- x\n", "1.8.0"), "");
        assert_eq!(notes("## [1x8x0]\n- x\n", "1.8.0"), "");
    }

    #[test]
    fn empty_section_has_no_notes() {
        assert_eq!(notes("# C\n\n## [2.0.0] - work in progress\n\n## [1.7.0] - x\n- a\n", "2.0.0"), "");
    }

    #[test]
    fn notes_stop_at_link_refs() {
        let t = "## [Unreleased]\n\n## [0.3.6] - WIP\n- a\n\n[Unreleased]: https://x/compare/v0.3.6...HEAD\n";
        assert_eq!(notes(t, "0.3.6"), "- a");
    }

    #[test]
    fn open_and_stamp() {
        assert!(is_open(SIMPLE, "1.8.0", "work in progress"));
        assert!(!is_open(SIMPLE, "1.7.0", "work in progress"));
        assert!(is_open("## [1.0.0]\n", "1.0.0", "WIP"));
        let s = stamp_date(SIMPLE, "1.8.0", "work in progress", "2026-10-18").unwrap();
        assert!(s.contains("## [1.8.0] - 2026-10-18\n"));
        assert!(!is_open(&s, "1.8.0", "work in progress"));
        assert!(stamp_date(&s, "1.8.0", "work in progress", "2026-10-19").is_none());
    }

    #[test]
    fn insert_top() {
        let cfg = ChangelogCfg::default();
        let s = insert_next(SIMPLE, &cfg, "1.8.0", "1.9.0").unwrap();
        assert!(s.starts_with("# Changelog\n\n## [1.9.0] - work in progress\n\n## [1.8.0]"));
    }

    #[test]
    fn insert_after_unreleased_with_links() {
        let cfg = ChangelogCfg {
            wip_marker: "WIP".into(),
            insert_next: InsertNext::AfterUnreleased,
            compare_links: Some("https://github.com/tinue/SharpDataExchange".into()),
            ..Default::default()
        };
        let t = "# C\n\n## [Unreleased]\n\n## [0.3.6] - 2026-10-20\n- a\n\n[Unreleased]: https://github.com/tinue/SharpDataExchange/compare/v0.3.6...HEAD\n[0.3.6]: https://github.com/tinue/SharpDataExchange/compare/v0.3.5...v0.3.6\n";
        let s = insert_next(t, &cfg, "0.3.6", "0.3.7").unwrap();
        assert!(s.contains("## [Unreleased]\n\n## [0.3.7] - WIP\n\n## [0.3.6]"));
        assert!(s.contains("[Unreleased]: https://github.com/tinue/SharpDataExchange/compare/v0.3.7...HEAD\n[0.3.7]: https://github.com/tinue/SharpDataExchange/compare/v0.3.6...v0.3.7\n[0.3.6]:"));
        assert!(insert_next("# C\n", &cfg, "1", "2").is_err());
    }

    #[test]
    fn real_changelogs() {
        let c59 = fixture("CHANGELOG-Calc-U-59.md");
        assert!(notes(&c59, "1.7.0").starts_with("- **Error messages**"));
        assert!(!is_open(&c59, "1.7.0", "work in progress"));

        let c1600 = fixture("CHANGELOG-Calc-U-1600.md");
        assert!(is_open(&c1600, "0.8.0", "work in progress"));
        assert!(!notes(&c1600, "0.7.0").is_empty());
        assert!(!notes(&c1600, "0.7.0").contains("## [0.6.0]"));

        let sde = fixture("CHANGELOG-SharpDataExchange.md");
        assert!(is_open(&sde, "0.3.6", "WIP"));
        let n = notes(&sde, "0.3.5");
        assert!(!n.is_empty());
        assert!(n.lines().all(|l| !l.starts_with("## ") && !l.contains("]: https://")));
        assert!(n.contains("### Changed"));

        let vp = fixture("CHANGELOG-VolumePruner.md");
        assert!(!notes(&vp, "1.1.0").is_empty());
    }
}
