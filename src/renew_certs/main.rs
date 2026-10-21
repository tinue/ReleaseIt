//! `renew-certs` — rotate the Developer ID signing certificates that CI
//! signs and notarizes with. See README.md.

mod cert;
mod config;
mod github;
mod keychain;
mod p12;
#[path = "../sh.rs"]
#[allow(dead_code)]
mod sh;

use anyhow::{Context, Result, bail};
use base64::Engine;
use cert::{CertInfo, KINDS, Kind, Rules};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use config::Config;
use keychain::Keychain;
use std::io::{BufRead, Write};
use std::path::PathBuf;

const PASSWORD_ENV: &str = "RENEW_CERTS_P12_PASSWORD";
const PORTAL_URL: &str = "https://developer.apple.com/account/resources/certificates/add";

/// Rotate the Developer ID certificates in CI:
///   renew-certs guide     how to create new certificates (manual, once a year)
///   renew-certs status    keychain identities and what each repo's CI has
///   renew-certs apply     export from the keychain, check, set the repo secrets
#[derive(Parser)]
#[command(name = "renew-certs", disable_version_flag = true, verbatim_doc_comment)]
struct Cli {
    /// Config file (default: ~/.config/releaseit/certs.yaml).
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,
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
    /// Upload new certificates to every configured repo.
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

/// Dry-run and confirmation handling, as in `release`.
struct Ui {
    dry_run: bool,
    yes: bool,
}

impl Ui {
    fn run(&self, prog: &str, args: &[&str]) -> Result<()> {
        if self.dry_run {
            println!("[dry-run] {}", sh::display(prog, args));
            Ok(())
        } else {
            sh::run(prog, args)
        }
    }

    fn confirm(&self, prompt: &str) -> Result<bool> {
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
}

fn section(title: &str) {
    let pad = 64usize.saturating_sub(title.chars().count() + 4);
    println!("\n── {title} {}", "─".repeat(pad));
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
    if let Cmd::Guide = cli.cmd {
        guide(&ui)?;
        return Ok(0);
    }
    let cfg = Config::load(&match cli.config {
        Some(p) => p,
        None => Config::default_path()?,
    })?;
    github::require()?;
    match cli.cmd {
        Cmd::Guide => unreachable!(),
        Cmd::Status => status(&cfg, &kc),
        Cmd::Apply { password, application, installer, only, force } => {
            let files = [(Kind::Application, application), (Kind::Installer, installer)]
                .into_iter()
                .filter_map(|(k, f)| f.map(|f| (k, f)))
                .collect();
            apply(&ui, &cfg, &kc, password, files, only, force)?;
            Ok(0)
        }
    }
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
                          checks them and sets the secrets in every repo.

Old certificates can simply expire; Developer ID certificates can't be
revoked in the portal, and already-notarized releases stay valid."
    );
    if !ui.dry_run && ui.confirm("Open Keychain Access and the developer portal now?")? {
        sh::run("open", &["-a", "Keychain Access"])?;
        sh::run("open", &[PORTAL_URL])?;
    }
    Ok(())
}

/// What a repo's CI holds for `kind`, from the repo variables.
struct CiCert {
    sha1: String,
    expires: DateTime<Utc>,
}

fn ci_cert(cfg: &Config, repo: &str, kind: Kind) -> Option<CiCert> {
    let (sha_var, exp_var) = cfg.variables(kind);
    let sha1 = github::variable(repo, &sha_var)?;
    let expires = DateTime::parse_from_rfc3339(&github::variable(repo, &exp_var)?).ok()?.with_timezone(&Utc);
    Some(CiCert { sha1, expires })
}

fn days_left(t: DateTime<Utc>) -> i64 {
    (t - Utc::now()).num_days()
}

fn status(cfg: &Config, kc: &Keychain) -> Result<i32> {
    let mut ci = Vec::new();
    for repo in &cfg.repos {
        for kind in KINDS {
            ci.push((repo.as_str(), kind, ci_cert(cfg, repo, kind)));
        }
    }

    section("Keychain");
    let ids = kc.identities()?;
    if ids.is_empty() {
        println!("No Developer ID identities (certificate + private key).");
    }
    for kind in KINDS {
        for id in ids.iter().filter(|i| i.info.kind == Some(kind)) {
            let used: Vec<&str> = ci
                .iter()
                .filter(|(_, k, c)| *k == kind && c.as_ref().is_some_and(|c| c.sha1 == id.info.sha1))
                .map(|(r, _, _)| *r)
                .collect();
            let team = if id.info.team == cfg.team_id { String::new() } else { format!("  team {}", id.info.team) };
            let ci_note = if used.is_empty() { String::new() } else { format!("  ← CI: {}", used.join(", ")) };
            println!("{:<26} {}{team}{}{ci_note}", kind.name(), id.info.describe(), expiry_note(id.info.not_after));
        }
    }

    section("CI");
    let mut attention = false;
    for (repo, kind, c) in &ci {
        match c {
            Some(c) => {
                let days = days_left(c.expires);
                attention |= days < cfg.min_days;
                println!(
                    "{repo:<28} {:<26} {}…  expires {}{}",
                    kind.name(),
                    &c.sha1[..8.min(c.sha1.len())],
                    c.expires.format("%Y-%m-%d"),
                    expiry_note(c.expires)
                );
            }
            None => {
                attention = true;
                println!("{repo:<28} {:<26} unknown (not recorded by renew-certs)", kind.name());
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
    cfg: &Config,
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

    // 1. The new identities, as .p12 encrypted with `pw`.
    section("Certificates");
    let mut sources = Vec::new();
    if files.is_empty() {
        let ids = kc.identities()?;
        let infos: Vec<CertInfo> = ids.iter().map(|i| i.info.clone()).collect();
        for kind in KINDS.into_iter().filter(|k| only.is_none_or(|o| o == *k)) {
            let Some(pick) = cert::pick(&infos, kind, &cfg.team_id) else {
                bail!(
                    "no G2 {} identity for team {} in the keychain.\n  Create one first: renew-certs guide",
                    kind.name(),
                    cfg.team_id
                );
            };
            let id = ids.iter().find(|i| i.info.sha1 == pick.sha1).context("picked identity")?;
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
        let ci_expires = cfg.repos.iter().filter_map(|r| ci_cert(cfg, r, kind)).map(|c| c.expires).max();
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
    println!("Repos: {}", cfg.repos.join(", "));
    if !both {
        println!(
            "Only one certificate: {} is left unchanged, so this .p12 must use the password CI already has.",
            cfg.secrets.password
        );
    }
    if !ui.confirm("Set the secrets?")? {
        bail!("aborted");
    }

    // 5. Secrets + variables.
    section("GitHub");
    for repo in &cfg.repos {
        for r in &renewals {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&r.p12);
            secret(ui, repo, cfg.secret(r.kind), b64.as_bytes())?;
            let (sha_var, exp_var) = cfg.variables(r.kind);
            let expires = r.info.not_after.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            ui.run("gh", &github::set_variable_args(repo, &sha_var, &r.info.sha1))?;
            ui.run("gh", &github::set_variable_args(repo, &exp_var, &expires))?;
        }
        if both {
            secret(ui, repo, &cfg.secrets.password, pw.as_bytes())?;
        }
    }

    // 6.+7. File input: keep the identity on this Mac, remove the export.
    let ids = kc.identities()?;
    for r in renewals.iter().filter(|r| r.file.is_some()) {
        let f = r.file.as_ref().unwrap();
        if !keychain::exists(&ids, &r.info.sha1) {
            if ui.dry_run {
                println!("[dry-run] import {} into the keychain", r.info.short_sha1());
            } else {
                kc.import(&r.p12, &pw)?;
                println!("Imported {} into the keychain.", r.info.short_sha1());
            }
        }
        if ui.dry_run {
            println!("[dry-run] rm {}", f.display());
        } else {
            std::fs::remove_file(f).with_context(|| format!("cannot delete {}", f.display()))?;
            println!("Deleted {}.", f.display());
        }
    }

    // 8. Older identities nothing uses any more.
    cleanup(ui, cfg, kc, &renewals)
}

fn secret(ui: &Ui, repo: &str, name: &str, value: &[u8]) -> Result<()> {
    if ui.dry_run {
        println!("[dry-run] gh secret set {name} -R {repo}  ({} bytes on stdin)", value.len());
        return Ok(());
    }
    github::set_secret(repo, name, value)?;
    println!("{repo}: {name} set");
    Ok(())
}

fn cleanup(ui: &Ui, cfg: &Config, kc: &Keychain, renewals: &[Renewal]) -> Result<()> {
    let referenced: Vec<String> = cfg
        .repos
        .iter()
        .flat_map(|r| KINDS.into_iter().filter_map(move |k| ci_cert(cfg, r, k)))
        .map(|c| c.sha1)
        .chain(renewals.iter().map(|r| r.info.sha1.clone()))
        .collect();
    let old: Vec<_> = kc
        .identities()?
        .into_iter()
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
            let mut args = vec!["delete-identity", "-Z", &i.info.sha1];
            let path;
            if let Some(p) = &kc.path {
                path = p.to_string_lossy().into_owned();
                args.push(&path);
            }
            ui.run("security", &args)?;
        }
    }
    Ok(())
}
