//! `renew-certs` — rotate the Developer ID signing certificates that a
//! project's CI signs and notarizes with. Configured in the `certs:` section
//! of the project's release.yaml. See README.md.

mod cert;
mod github;
mod keychain;
mod onepassword;
mod p12;

use anyhow::{Context, Result};
use base64::Engine;
use cert::{CertInfo, KINDS, Kind, Rules};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use keychain::{Identity, Keychain};
use releaseit::config::{self, CertsCfg, Config};
use releaseit::ui::{Ui, section};
use releaseit::{gh, sh};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const CERTS_SAMPLE: &str = "\
certs:
  team_id: ABCDE12345     # Apple team ID (the certificate subject's OU)
  # min_days: 30
  # secrets:              # defaults:
  #   application: APPLE_CERT_APPLICATION_P12_BASE64
  #   installer:   APPLE_CERT_INSTALLER_P12_BASE64
  #   password:    APPLE_CERT_P12_PASSWORD
";
const PORTAL_URL: &str = "https://developer.apple.com/account/resources/certificates/add";

/// Rotate the Developer ID certificates in CI:
///   renew-certs guide     how to create new certificates (manual, once a year)
///   renew-certs status    keychain identities, their backups, what this repo's CI has
///   renew-certs apply     export from the keychain, check, set this repo's secrets
///   renew-certs backup    store the current identities in 1Password (Private vault)
#[derive(Parser)]
#[command(name = "renew-certs", disable_version_flag = true, verbatim_doc_comment)]
struct Cli {
    /// Run as if started in this directory.
    #[arg(short = 'C', global = true, value_name = "DIR")]
    dir: Option<PathBuf>,
    /// Keychain to use instead of the default search list.
    #[arg(long, global = true, value_name = "FILE")]
    keychain: Option<PathBuf>,
    /// Print state-changing actions instead of running them.
    #[arg(long, global = true)]
    dry_run: bool,
    /// Answer yes to every confirmation prompt.
    #[arg(long, short = 'y', global = true)]
    yes: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Explain how to create the certificates (developer portal, G2 Sub-CA).
    Guide,
    /// List keychain identities, their backups and the certificates CI uses; exit 1 if CI's expire soon.
    Status,
    /// Upload both certificates to this repo's secrets, under a new random password.
    Apply {
        /// Allow a certificate that expires earlier than the one CI has.
        #[arg(long)]
        force: bool,
    },
    /// Store the current Application and Installer identities in 1Password.
    Backup,
}

fn main() {
    match real_main() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
    }
}

fn real_main() -> Result<i32> {
    let cli = Cli::parse();
    let ui = Ui { dry_run: cli.dry_run, yes: cli.yes };
    let kc = Keychain { path: cli.keychain };
    match cli.cmd {
        Cmd::Guide => guide(&ui).map(|()| 0),
        Cmd::Status => status(&load(cli.dir.as_deref())?, &kc),
        Cmd::Apply { force } => apply(&ui, &load(cli.dir.as_deref())?, &kc, force).map(|()| 0),
        Cmd::Backup => backup(&ui, &load(cli.dir.as_deref())?, &kc).map(|()| 0),
    }
}

/// The project's `certs:` settings, from release.yaml at the repo root.
fn load(dir: Option<&Path>) -> Result<CertsCfg> {
    let root = sh::enter_repo_root(dir)?;
    gh::require()?;
    Config::load(&root)?
        .certs
        .with_context(|| format!("no `certs:` section in {}. Add e.g.:\n\n{CERTS_SAMPLE}", config::FILE_NAME))
}

fn guide(ui: &Ui) -> Result<()> {
    println!(
        "\
Developer ID certificates from the G2 Sub-CA are valid ~13 months. To renew:

1. Keychain Access ▸ Certificate Assistant ▸ Request a Certificate From a
   Certificate Authority… — your email and name, \"Saved to disk\".
   Make a NEW request for each certificate: Apple rejects a CSR it has
   already used (\"The uploaded CSR file has already been used…\").

2. {PORTAL_URL}
   ▸ Developer ID Application (then again: Developer ID Installer)
   ▸ Sub-CA: \"G2 Sub-CA (Xcode 11.4.1 or later)\" ▸ upload the CSR.
   Do NOT use Xcode ▸ Settings ▸ Accounts ▸ Manage Certificates: it issued
   certificates from the original Sub-CA, which expires 2027-02-01.
   Creating them needs the Account Holder role.

3. Download each .cer and double-click it; it joins its private key in the
   login keychain.

4. renew-certs status   — the new identities show as G2
   renew-certs apply    — exports both (macOS asks for the keychain password
                          twice per certificate, so four times), checks
                          them and sets the secrets. Run it in the
                          root of every project whose release.yaml has a
                          `certs:` section.
   renew-certs backup   — stores both in 1Password (Private vault); the
                          same four keychain prompts, plus 1Password's.

Old certificates can simply expire; Developer ID certificates can't be
revoked in the portal, and already-notarized releases stay valid.
To restore a backup on another Mac: download the .p12 attached to the
1Password item, double-click it and enter the item's password."
    );
    if !ui.dry_run && ui.confirm("Open Keychain Access and the developer portal now?")? {
        sh::run("open", &["-a", "Keychain Access"])?;
        sh::run("open", &[PORTAL_URL])?;
    }
    Ok(())
}

fn secret_name(cfg: &CertsCfg, kind: Kind) -> &str {
    match kind {
        Kind::Application => &cfg.secrets.application,
        Kind::Installer => &cfg.secrets.installer,
    }
}

/// Repo variables recording what a secret currently holds, so `status` can
/// report it (secrets can't be read back):
/// APPLE_CERT_APPLICATION_P12_BASE64 → APPLE_CERT_APPLICATION_SHA1 / _EXPIRES.
fn variables(cfg: &CertsCfg, kind: Kind) -> (String, String) {
    let s = secret_name(cfg, kind);
    let base = s.strip_suffix("_P12_BASE64").unwrap_or(s);
    (format!("{base}_SHA1"), format!("{base}_EXPIRES"))
}

/// What the repo's CI holds for `kind`, from the repo variables.
struct CiCert {
    sha1: String,
    expires: DateTime<Utc>,
}

fn ci_cert(cfg: &CertsCfg, vars: &HashMap<String, String>, kind: Kind) -> Option<CiCert> {
    let (sha_var, exp_var) = variables(cfg, kind);
    let sha1 = vars.get(&sha_var)?.clone();
    let expires = DateTime::parse_from_rfc3339(vars.get(&exp_var)?).ok()?.with_timezone(&Utc);
    Some(CiCert { sha1, expires })
}

/// The 1Password item title for a certificate, a key-style name like the
/// home-ca items: developer-id-application-2027-11-20-47c2819d.
fn backup_title(c: &CertInfo) -> String {
    let kind = c.kind.map(Kind::name).unwrap_or("developer-id").to_lowercase().replace(' ', "-");
    format!("{kind}-{}-{}", c.expires(), c.short_sha1().to_lowercase())
}

/// The newest G2 identity of each type, or an error naming what's missing.
fn current<'a>(cfg: &CertsCfg, ids: &'a [Identity]) -> Result<Vec<(Kind, &'a Identity)>> {
    KINDS
        .into_iter()
        .map(|kind| {
            cert::pick(ids, kind, &cfg.team_id).map(|id| (kind, id)).with_context(|| {
                format!(
                    "no G2 {} identity for team {} in the keychain.\n  Create one first: renew-certs guide",
                    kind.name(),
                    cfg.team_id
                )
            })
        })
        .collect()
}

fn days_left(t: DateTime<Utc>) -> i64 {
    (t - Utc::now()).num_days()
}

fn status(cfg: &CertsCfg, kc: &Keychain) -> Result<i32> {
    let vars = github::variables()?;
    let ci: Vec<(Kind, Option<CiCert>)> = KINDS.into_iter().map(|k| (k, ci_cert(cfg, &vars, k))).collect();
    // A locked or missing 1Password only hides the backup column.
    let backups = onepassword::items().map_err(|e| println!("(1Password: {e:#})")).ok();

    section("Keychain");
    let ids = kc.identities()?;
    if ids.is_empty() {
        println!("No Developer ID identities (certificate + private key).");
    }
    for kind in KINDS {
        for id in ids.iter().filter(|i| i.info.kind == Some(kind)) {
            let used = ci.iter().any(|(k, c)| *k == kind && c.as_ref().is_some_and(|c| c.sha1 == id.info.sha1));
            let team = if id.info.team == cfg.team_id { String::new() } else { format!("  team {}", id.info.team) };
            let ci_note = if used { "  ← CI" } else { "" };
            let backup = match &backups {
                Some(b) if id.info.g2 && b.iter().any(|i| i.title == backup_title(&id.info)) => "  backed up",
                Some(_) if id.info.g2 => "  NO BACKUP",
                _ => "",
            };
            println!(
                "{:<26} {}{team}{}{ci_note}{backup}",
                kind.name(),
                id.info.describe(),
                expiry_note(id.info.not_after)
            );
        }
    }

    section("CI (repo variables)");
    let mut attention = false;
    for (kind, c) in &ci {
        match c {
            Some(c) => {
                attention |= days_left(c.expires) < cfg.min_days;
                println!(
                    "{:<26} {}…  expires {}{}",
                    kind.name(),
                    cert::short(&c.sha1),
                    c.expires.format("%Y-%m-%d"),
                    expiry_note(c.expires)
                );
            }
            None => {
                attention = true;
                println!("{:<26} unknown (not recorded by renew-certs)", kind.name());
            }
        }
    }
    if attention {
        println!("\nAction needed: run `renew-certs guide`, then `renew-certs apply`.");
    }
    Ok(i32::from(attention))
}

fn expiry_note(t: DateTime<Utc>) -> String {
    match days_left(t) {
        d if d < 0 => "  EXPIRED".into(),
        d if d < 60 => format!("  ({d} days left)"),
        _ => String::new(),
    }
}

struct Renewal {
    kind: Kind,
    info: CertInfo,
    p12: Vec<u8>,
}

fn apply(ui: &Ui, cfg: &CertsCfg, kc: &Keychain, force: bool) -> Result<()> {
    let vars = github::variables()?;
    let ids = kc.identities()?;

    // 1. Both identities, exported under one new random password: CI imports
    // both .p12s with the one password secret.
    section("Certificates");
    let pw = keychain::random_password()?;
    let mut renewals = Vec::new();
    for (kind, id) in current(cfg, &ids)? {
        println!("{:<26} keychain {}", kind.name(), id.info.describe());
        let p12 = id.export(&pw)?;
        // 2. What CI will import: one certificate with its key, and the rules.
        let info = p12::inspect(&p12, &pw).with_context(|| format!("exported {}", kind.name()))?;
        let ci_expires = ci_cert(cfg, &vars, kind).map(|c| c.expires);
        if ci_expires.is_none() {
            println!("  note: no {} recorded for CI yet; skipping the downgrade check", kind.name());
        }
        Rules { kind, team: &cfg.team_id, min_days: cfg.min_days, now: Utc::now(), ci_expires, force }.check(&info)?;
        renewals.push(Renewal { kind, info, p12 });
    }

    // 3. Summary.
    section("Plan");
    for r in &renewals {
        println!("{:<26} {}  {}", r.kind.name(), r.info.describe(), r.info.common_name);
    }
    println!("{}: a new random password (kept nowhere else; re-run apply to replace it)", cfg.secrets.password);
    ui.require_confirm("Set the secrets?")?;

    // 4. Secrets + variables.
    section("GitHub");
    for r in &renewals {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&r.p12);
        github::set_secret(ui, secret_name(cfg, r.kind), b64.as_bytes())?;
        let (sha_var, exp_var) = variables(cfg, r.kind);
        let expires = r.info.not_after.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        github::set_variable(ui, &sha_var, &r.info.sha1)?;
        github::set_variable(ui, &exp_var, &expires)?;
    }
    github::set_secret(ui, &cfg.secrets.password, pw.as_bytes())?;

    // 5. Older identities: the new ones are what CI has now.
    let new: Vec<&str> = renewals.iter().map(|r| r.info.sha1.as_str()).collect();
    cleanup(ui, cfg, kc, &ids, &renewals, &new)?;

    println!("\nDone. Back up the new identities, if not done yet: renew-certs backup");
    Ok(())
}

/// Offer to delete superseded identities from the keychain. CI is unaffected
/// either way (it imports its own copy from the secret); other projects
/// renew from the newest identity, which is never offered.
fn cleanup(
    ui: &Ui,
    cfg: &CertsCfg,
    kc: &Keychain,
    ids: &[Identity],
    renewals: &[Renewal],
    keep: &[&str],
) -> Result<()> {
    let old: Vec<&Identity> = ids
        .iter()
        .filter(|i| {
            renewals.iter().any(|r| {
                // Same type and team, and superseded: from the original
                // Sub-CA (dead after 2027-02-01) or expiring before the new one.
                i.info.kind == Some(r.kind)
                    && i.info.team == cfg.team_id
                    && (!i.info.g2 || i.info.not_after < r.info.not_after)
            }) && !keep.contains(&i.info.sha1.as_str())
        })
        .collect();
    if old.is_empty() {
        return Ok(());
    }
    section("Old identities");
    for i in &old {
        let kind = i.info.kind.map(Kind::name).unwrap_or_default();
        if ui.confirm(&format!("Delete {kind} {} from the keychain?", i.info.describe()))? {
            kc.delete(ui, &i.info.sha1)?;
        }
    }
    Ok(())
}

/// Store each current identity as a `.p12` in its own 1Password item. An
/// item that exists with its attachment is left alone, so this can re-run.
fn backup(ui: &Ui, cfg: &CertsCfg, kc: &Keychain) -> Result<()> {
    let ids = kc.identities()?;
    let existing = onepassword::items()?;
    let host = sh::try_capture("scutil", &["--get", "ComputerName"]).unwrap_or_else(|| "this Mac".into());
    let dir = tempfile::tempdir()?;
    section(&format!("1Password backup (vault {})", onepassword::VAULT));
    for (kind, id) in current(cfg, &ids)? {
        let title = backup_title(&id.info);
        let item = match existing.iter().find(|i| i.title == title) {
            Some(i) => {
                let item = onepassword::get(&i.id)?;
                if item.has_file {
                    println!("{title}: already backed up");
                    continue;
                }
                Some(item)
            }
            None => None,
        };
        let notes = format!(
            "{} for team {}, SHA-1 {}, expires {}. Exported by renew-certs on {host}, {}.\n\
             Restore: download the attached .p12, double-click it and enter this item's password; \
             then run `renew-certs apply` in each project.",
            kind.name(),
            id.info.team,
            id.info.sha1,
            id.info.expires(),
            Utc::now().format("%Y-%m-%d")
        );
        ui.act(&format!("create 1Password item {title} with the .p12 attached"), || {
            let item = match item {
                Some(item) => item,
                None => onepassword::create(&title, &notes)?,
            };
            let p12 = id.export(&item.password)?;
            p12::inspect(&p12, &item.password).with_context(|| format!("exported {}", kind.name()))?;
            // op attaches only from a file: write it to the private temp dir,
            // removed when this command ends.
            let file = dir.path().join(format!("{title}.p12"));
            std::fs::write(&file, &p12)?;
            onepassword::attach(&item.id, &file)?;
            std::fs::remove_file(&file)?;
            println!("{title}: backed up");
            Ok(())
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_names_follow_the_secret_names() {
        let cfg = Config::parse("name: X\nversion: { preset: cargo }\ncerts: { team_id: T }\n").unwrap().certs.unwrap();
        assert_eq!(secret_name(&cfg, Kind::Installer), "APPLE_CERT_INSTALLER_P12_BASE64");
        assert_eq!(
            variables(&cfg, Kind::Application),
            ("APPLE_CERT_APPLICATION_SHA1".into(), "APPLE_CERT_APPLICATION_EXPIRES".into())
        );
    }

    #[test]
    fn sample_parses() {
        let yaml = format!("name: X\nversion: {{ preset: cargo }}\n{CERTS_SAMPLE}");
        assert_eq!(Config::parse(&yaml).unwrap().certs.unwrap().team_id, "ABCDE12345");
    }

    #[test]
    fn backup_titles_are_key_style() {
        let der = x509_parser::pem::Pem::iter_from_buffer(
            include_str!("../../tests/fixtures/certs/application-g2.pem").as_bytes(),
        )
        .next()
        .unwrap()
        .unwrap()
        .contents;
        let c = CertInfo::from_der(&der).unwrap();
        let t = backup_title(&c);
        assert!(t.starts_with(&format!("developer-id-application-{}-", c.expires())), "{t}");
        assert!(t.ends_with(&c.short_sha1().to_lowercase()));
    }
}
