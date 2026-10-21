//! `kind: xcode-dmg` — the macOS app, built locally: Xcode archive →
//! Developer ID export → DMG → sign → (pre-release only) notarize + staple.

use super::xcode::{self, notarize, signing_identity};
use crate::config::XcodeDmg;
use crate::ctx::Ctx;
use crate::sh;
use anyhow::{Result, bail};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub fn build(ctx: &Ctx, x: &XcodeDmg, notarize_it: bool) -> Result<Vec<PathBuf>> {
    let app = x.app_name.as_deref().unwrap_or(&x.scheme);
    let dir = ctx.root.join(&x.build_dir);
    let dmg = dir.join(format!("{app}-{}.dmg", ctx.version));
    if ctx.dry_run {
        println!(
            "[dry-run] would archive {}, export with Developer ID, create + sign {}{}",
            x.scheme,
            dmg.display(),
            if notarize_it { ", notarize + staple" } else { "" }
        );
        return Ok(vec![dmg]);
    }

    xcode::require_xcode()?;
    sh::require_tool("create-dmg", "Install with: brew install create-dmg")?;
    let identity = signing_identity()?;
    println!("Using signing identity: {identity}");

    println!("Archiving {app} {}…", ctx.version);
    // arch=arm64 in the destination pins the build to Apple Silicon and avoids
    // the "multiple matching destinations" warning when Rosetta is present;
    // ARCHS=arm64 makes the archive itself contain only the arm64 slice.
    //
    // The export re-signs with the Developer ID certificate AND embeds the
    // matching Developer ID provisioning profile, which entitlements such as
    // iCloud need at runtime. A plain `codesign --force` re-signs the binary
    // but keeps the archive's development profile, and AMFI then rejects the
    // entitlements.
    let export = xcode::archive_and_export(
        &x.project,
        &x.scheme,
        app,
        "platform=macOS,arch=arm64",
        &["ARCHS=arm64"],
        &x.export_options,
        &dir,
    )?;
    let exported = export.join(format!("{app}.app"));
    if !exported.is_dir() {
        return Err(xcode::not_exported(&export, &format!("{app}.app")));
    }
    let app_path = dir.join(format!("{app}.app"));
    sh::run("cp", &["-R", sh::path_str(&exported)?, sh::path_str(&app_path)?])?;
    sh::run("codesign", &["--verify", "--deep", "--strict", sh::path_str(&app_path)?])?;
    println!("Exported and signed: {}", app_path.display());

    println!("Creating DMG…");
    create_dmg(ctx, app, &app_path, &dmg)?;
    sh::run("codesign", &["--force", "--sign", &identity, sh::path_str(&dmg)?])?;
    println!("DMG: {}", dmg.display());

    if notarize_it {
        notarize(&x.notary_profile, &dmg)?;
    }
    Ok(vec![dmg])
}

fn create_dmg(ctx: &Ctx, app: &str, app_path: &Path, dmg: &Path) -> Result<()> {
    let volname = format!("{app} {}", ctx.version);
    let volicon = app_path.join("Contents/Resources/AppIcon.icns");
    let icon = format!("{app}.app");
    let args = [
        "--volname", &volname, "--volicon", sh::path_str(&volicon)?,
        "--window-pos", "200", "120", "--window-size", "660", "400", "--icon-size", "128",
        "--icon", &icon, "165", "185", "--app-drop-link", "495", "185", "--no-internet-enable",
        sh::path_str(dmg)?, sh::path_str(app_path)?,
    ];
    let finder_blocked = regex::Regex::new(r"\(-1743\)|Apple[ -]Events.*Finder|Finder.*Apple[ -]Events").unwrap();
    loop {
        let (ok, out) = sh::capture_all("create-dmg", &args)?;
        print!("{out}");
        if ok {
            return Ok(());
        }
        // create-dmg drives Finder via AppleScript to lay out the window; macOS
        // blocks that until the terminal app is granted Automation → Finder.
        if !finder_blocked.is_match(&out) || ctx.yes {
            bail!("create-dmg failed");
        }
        println!("\nmacOS blocked Apple Events automation for Finder (error -1743).");
        println!("Grant permission for your terminal app:");
        println!("  1) Open System Settings → Privacy & Security → Automation");
        println!("  2) Select your terminal app (e.g. iTerm)");
        println!("  3) Enable Finder\n");
        print!("After granting permission, press Enter to retry DMG creation (or type 'abort' to stop): ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        if line.trim().eq_ignore_ascii_case("abort") {
            bail!("aborted");
        }
        let _ = std::fs::remove_file(dmg);
        println!("Retrying DMG creation…");
    }
}
