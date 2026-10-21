//! `renew-certs` — rotate the Developer ID signing certificates that a
//! project's CI signs and notarizes with. Configured in the `certs:` section
//! of the project's release.yaml. See README.md.

mod cert;
mod github;
mod keychain;
mod p12;

use anyhow::{Context, Result, bail};
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

const PASSWORD_ENV: &str = "RENEW_CERTS_P12_PASSWORD";
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
///   renew-certs status    keychain identities and what this repo's CI has
///   renew-certs apply     export from the keychain, check, set this repo's secrets
#[derive(Parser)]
#[command(name = "renew-certs", disable_version_flag = true, verbatim_doc_comment)]
struct Cli {
    /// Run as if started in this directory.
    #[arg(short = 'C', global = true, value_name = "DIR")]
    dir: Option<PathBuf>,
    /// Keychain to use instead of the default search list / login keychain.
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
    /// List keychain identities and the certificates CI uses; exit 1 if CI's expire soon.
    Status,
    /// Upload new certificates to this repo's secrets.
    Apply {
        /// Password for the .p12s CI imports (else $RENEW_CERTS_P12_PASSWORD, else prompt).
        #[arg(long)]
        password: Option<String>,
        /// Take the Application identity from this .p12 instead of the keychain.
        #[arg(long, value_name = "FILE")]
        application: Option<PathBuf>,
        /// Take the Installer identity from this .p12 instead of the keychain.
        #[arg(long, value_name = "FILE")]
        installer: Option<PathBuf>,
        /// Renew only this certificate type (keychain source).
        #[arg(long, value_enum, conflicts_with_all = ["application", "installer"])]
        only: Option<Kind>,
        /// Allow a certificate that expires earlier than the one CI has.
        #[arg(long)]
        force: bool,
    },
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
        Cmd::Apply { password, application, installer, only, force } => {
            let cfg = load(cli.dir.as_deref())?;
            let files = [(Kind::Application, application), (Kind::Installer, installer)]
                .into_iter()
                .filter_map(|(k, f)| f.map(|f| (k, f)))
                .collect();
            apply(&ui, &cfg, &kc, password, files, only, force).map(|()| 0)
        }
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
   renew-certs apply    — exports them (macOS asks to allow the key export),
                          checks them and sets the secrets. Run it in the
                          root of every project whose release.yaml has a
                          `certs:` section.

Old certificates can simply expire; Developer ID certificates can't be
revoked in the portal, and already-notarized releases stay valid."
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

fn days_left(t: DateTime<Utc>) -> i64 {
    (t - Utc::now()).num_days()
}

fn status(cfg: &CertsCfg, kc: &Keychain) -> Result<i32> {
    let vars = github::variables()?;
    let ci: Vec<(Kind, Option<CiCert>)> = KINDS.into_iter().map(|k| (k, ci_cert(cfg, &vars, k))).collect();

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
            println!("{:<26} {}{team}{}{ci_note}", kind.name(), id.info.describe(), expiry_note(id.info.not_after));
        }
    }

    section("CI (repo variables)");
    let mut attention = false;
    for (kind, c) in &ci {
        match c {
            Some(c) => {
                let days = days_left(c.expires);
                attention |= days < cfg.min_days;
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

fn password(given: Option<String>) -> Result<String> {
    if let Some(p) = given.or_else(|| std::env::var(PASSWORD_ENV).ok()) {
        return Ok(p);
    }
    let p = rpassword::prompt_password(".p12 password for CI: ")?;
    if rpassword::prompt_password("Repeat: ")? != p {
        bail!("passwords differ");
    }
    Ok(p)
}

struct Renewal {
    kind: Kind,
    info: CertInfo,
    p12: Vec<u8>,
    file: Option<PathBuf>,
}

fn apply(
    ui: &Ui,
    cfg: &CertsCfg,
    kc: &Keychain,
    password: Option<String>,
    files: Vec<(Kind, PathBuf)>,
    only: Option<Kind>,
    force: bool,
) -> Result<()> {
    let pw = self::password(password)?;
    if pw.is_empty() {
        bail!("the .p12 password must not be empty");
    }

    let vars = github::variables()?;
    let ids = kc.identities()?;

    // 1. The new identities, as .p12 encrypted with `pw`.
    section("Certificates");
    let mut sources = Vec::new();
    if files.is_empty() {
        for kind in KINDS.into_iter().filter(|k| only.is_none_or(|o| o == *k)) {
            let Some(id) = cert::pick(&ids, kind, &cfg.team_id) else {
                bail!(
                    "no G2 {} identity for team {} in the keychain.\n  Create one first: renew-certs guide",
                    kind.name(),
                    cfg.team_id
                );
            };
            println!("{:<26} keychain {}", kind.name(), id.info.describe());
            sources.push((kind, id.export(&pw)?, None));
        }
    } else {
        for (kind, f) in files {
            let p12 = std::fs::read(&f).with_context(|| format!("cannot read {}", f.display()))?;
            println!("{:<26} {}", kind.name(), f.display());
            sources.push((kind, p12, Some(f)));
        }
    }

    // 2.+3. Contents and rules.
    let mut renewals = Vec::new();
    for (kind, p12, file) in sources {
        let info = p12::inspect(&p12, &pw).with_context(|| {
            format!("{}{}", kind.name(), file.as_ref().map(|f| format!(" ({})", f.display())).unwrap_or_default())
        })?;
        let ci_expires = ci_cert(cfg, &vars, kind).map(|c| c.expires);
        if ci_expires.is_none() {
            println!("  note: no {} recorded for CI yet; skipping the downgrade check", kind.name());
        }
        Rules { kind, team: &cfg.team_id, min_days: cfg.min_days, now: Utc::now(), ci_expires, force }.check(&info)?;
        renewals.push(Renewal { kind, info, p12, file });
    }
    let both = renewals.len() == KINDS.len();

    // 4. Summary.
    section("Plan");
    for r in &renewals {
        println!("{:<26} {}  {}", r.kind.name(), r.info.describe(), r.info.common_name);
    }
    if !both {
        println!(
            "Only one certificate: {} is left unchanged, so this .p12 must use the password CI already has.",
            cfg.secrets.password
        );
    }
    ui.require_confirm("Set the secrets?")?;

    // 5. Secrets + variables.
    section("GitHub");
    for r in &renewals {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&r.p12);
        github::set_secret(ui, secret_name(cfg, r.kind), b64.as_bytes())?;
        let (sha_var, exp_var) = variables(cfg, r.kind);
        let expires = r.info.not_after.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        github::set_variable(ui, &sha_var, &r.info.sha1)?;
        github::set_variable(ui, &exp_var, &expires)?;
    }
    if both {
        github::set_secret(ui, &cfg.secrets.password, pw.as_bytes())?;
    }

    // 6.+7. File input: keep the identity on this Mac, remove the export.
    for r in &renewals {
        let Some(f) = &r.file else { continue };
        if !ids.iter().any(|i| i.info.sha1 == r.info.sha1) {
            ui.act(&format!("import {} into the keychain", r.info.short_sha1()), || kc.import(&r.p12, &pw))?;
        }
        ui.act(&format!("rm {}", f.display()), || {
            std::fs::remove_file(f).with_context(|| format!("cannot delete {}", f.display()))
        })?;
    }

    // 8. Older identities nothing uses any more: what CI keeps for the
    // types not renewed, plus the new ones, stays.
    let referenced: Vec<String> = KINDS
        .into_iter()
        .filter(|k| renewals.iter().all(|r| r.kind != *k))
        .filter_map(|k| ci_cert(cfg, &vars, k))
        .map(|c| c.sha1)
        .chain(renewals.iter().map(|r| r.info.sha1.clone()))
        .collect();
    cleanup(ui, cfg, kc, &ids, &renewals, &referenced)
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
    referenced: &[String],
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
            }) && !referenced.contains(&i.info.sha1)
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
}
