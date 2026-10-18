//! `kind: xcode-dmg` — the macOS app, built locally: Xcode archive →
//! Developer ID export → DMG → sign → (pre-release only) notarize + staple.

use super::xcode::{self, notarize, signing_identity, xcodebuild};
use crate::config::XcodeDmg;
use crate::ctx::Ctx;
use crate::sh;
use anyhow::{Context, Result, bail};
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
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let archive = dir.join(format!("{app}.xcarchive"));
    // arch=arm64 in the destination pins the build to Apple Silicon and avoids
    // the "multiple matching destinations" warning when Rosetta is present;
    // ARCHS=arm64 makes the archive itself contain only the arm64 slice.
    xcodebuild(
        &[
            "archive", "-project", s(&x.project)?, "-scheme", &x.scheme, "-configuration", "Release",
            "-destination", "platform=macOS,arch=arm64", "-archivePath", s(&archive)?, "ARCHS=arm64",
        ],
        &["Build", "Archive"],
    )?;

    // -exportArchive re-signs with the Developer ID certificate AND embeds the
    // matching Developer ID provisioning profile, which entitlements such as
    // iCloud need at runtime. A plain `codesign --force` re-signs the binary
    // but keeps the archive's development profile, and AMFI then rejects the
    // entitlements. -allowProvisioningUpdates lets Xcode fetch that profile.
    let export = dir.join("export");
    println!("Exporting with Developer ID…");
    xcodebuild(
        &[
            "-exportArchive", "-archivePath", s(&archive)?, "-exportPath", s(&export)?,
            "-exportOptionsPlist", s(&x.export_options)?, "-allowProvisioningUpdates",
        ],
        &["Export"],
    )?;
    let exported = export.join(format!("{app}.app"));
    if !exported.is_dir() {
        let _ = sh::run("find", &[s(&export)?, "-maxdepth", "3"]);
        bail!("exported .app not found at {}", exported.display());
    }
    let app_path = dir.join(format!("{app}.app"));
    sh::run("cp", &["-R", s(&exported)?, s(&app_path)?])?;
    sh::run("codesign", &["--verify", "--deep", "--strict", s(&app_path)?])?;
    println!("Exported and signed: {}", app_path.display());

    println!("Creating DMG…");
    create_dmg(ctx, app, &app_path, &dmg)?;
    sh::run("codesign", &["--force", "--sign", &identity, s(&dmg)?])?;
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
        "--volname", &volname, "--volicon", s(&volicon)?,
        "--window-pos", "200", "120", "--window-size", "660", "400", "--icon-size", "128",
        "--icon", &icon, "165", "185", "--app-drop-link", "495", "185", "--no-internet-enable",
        s(dmg)?, s(app_path)?,
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

fn s(p: &Path) -> Result<&str> {
    p.to_str().context("non-UTF-8 path")
}
