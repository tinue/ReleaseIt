//! `release ios` — iOS/iPadOS via App Store Connect. No promote step of its
//! own; App Store Connect handles staging.
//!
//!   release ios --build-only   archive + export the IPA, nothing else
//!   release ios                TestFlight: build, validate, upload
//!   release ios --release      set up the App Store version: upload the
//!                              reviewed metadata, What's New and screenshots
//!                              and attach the build uploaded earlier. The only
//!                              manual step left is "Submit for Review".
//!
//! Metadata lives in the `appstore` worktree (an orphan branch, gitignored on
//! the main branches): metadata/*.txt, screenshots/iphone-NN*.png and
//! ipad-NN*.png, and whatsnew.txt (written by the macOS pre-release).
//! One-time setup per clone: git worktree add appstore appstore
//!
//! Environment: APP_STORE_API_KEY_ID, APP_STORE_API_ISSUER_ID (see asc.rs),
//! SKIP_VALIDATE=1 skips `altool --validate-app` (offline, or Apple's
//! validation service is down).

use super::asc::{Asc, Credentials};
use crate::build::xcode::{self, xcodebuild};
use crate::config::AppStoreCfg;
use crate::ctx::{self, Ctx};
use crate::git::{self, StashGuard};
use crate::{gh, sh};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Screenshot sets uploaded by --release: file prefix, ASC display type,
/// required pixel size (either orientation).
const SCREENSHOT_SETS: &[(&str, &str, (u32, u32))] = &[
    ("iphone-", "APP_IPHONE_67", (1320, 2868)),
    ("ipad-", "APP_IPAD_PRO_3GEN_129", (2752, 2064)),
];

struct Paths {
    project: PathBuf,
    scheme: String,
    app_name: String,
    metadata: PathBuf,
    screenshots: PathBuf,
    whatsnew: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx, app: &AppStoreCfg) -> Result<Self> {
        let mac = ctx.cfg.xcode_dmg();
        let project = app
            .project
            .clone()
            .or_else(|| mac.map(|m| m.project.clone()))
            .context("appstore.project is required when there is no xcode-dmg build step")?;
        let scheme = app
            .scheme
            .clone()
            .or_else(|| mac.map(|m| m.scheme.clone()))
            .context("appstore.scheme is required when there is no xcode-dmg build step")?;
        let app_name = mac.and_then(|m| m.app_name.clone()).unwrap_or_else(|| scheme.clone());
        let wt = ctx.root.join(&app.worktree);
        Ok(Self {
            project,
            scheme,
            app_name,
            metadata: wt.join("metadata"),
            screenshots: wt.join("screenshots"),
            whatsnew: wt.join("whatsnew.txt"),
        })
    }
}

pub fn run(ctx: &Ctx, build_only: bool, release: bool) -> Result<()> {
    let app = ctx.cfg.appstore.as_ref().context("release.yaml has no appstore: section")?;
    let p = Paths::new(ctx, app)?;

    xcode::require_xcode()?;
    if !sh::succeeds("xcrun", &["altool", "--version"]) {
        bail!("xcrun altool not found. Ensure Xcode is installed and selected.");
    }
    let mut creds = None;
    if !build_only {
        gh::require()?;
        if !p.metadata.join("description.txt").is_file() {
            bail!(
                "App Store metadata worktree not found at '{}/'. Set it up once with:\n  git worktree add {0} {0}",
                app.worktree.display()
            );
        }
        let c = Credentials::from_env()?;
        // Skipped under --dry-run so a preview works without credentials installed.
        if !ctx.dry_run {
            c.require_key_file()?;
        }
        creds = Some(c);
    }

    println!("Preparing {} {} for the App Store…", p.app_name, ctx.version);
    let mut _stash = None;
    let mut notes = String::new();
    if !build_only {
        let branch = git::current_branch()?;
        git::require_not_main(ctx, &branch, "An App Store release")?;
        _stash = Some(StashGuard::acquire(ctx)?);
        git::require_clean_index()?;
        let upstream = git::upstream(&branch)?;
        git::require_in_sync(&branch, &upstream)?;
        // iOS and macOS must ship from the same code: a macOS GitHub release
        // (pre-release or final) must exist for this version.
        if gh::view(&ctx.pre_tag).is_none() && gh::view(&ctx.tag).is_none() {
            bail!(
                "no GitHub release {} or {} found.\n  Publish the macOS pre-release first: release",
                ctx.pre_tag,
                ctx.tag
            );
        }
        notes = ctx.notes()?;
        if notes.is_empty() {
            bail!("no CHANGELOG entry for {}. Add one before releasing.", ctx.version);
        }
    }

    if ctx.dry_run {
        if p.whatsnew.is_file() && std::fs::metadata(&p.whatsnew)?.len() > 0 {
            ctx::section("App Store release notes (whatsnew.txt)");
            print!("{}", std::fs::read_to_string(&p.whatsnew)?);
            ctx::end_section();
        } else if !notes.is_empty() {
            ctx::section("CHANGELOG entry (whatsnew.txt not yet generated)");
            println!("{notes}");
            ctx::end_section();
            println!("  (`release` generates the plain-text App Store version)");
        }
        if release {
            println!("[dry-run] would review metadata + screenshots, verify the uploaded build, then set up App Store version {}", ctx.version);
        } else {
            println!("[dry-run] would archive, export, {}upload the IPA", if build_only { "(build only — no) " } else { "validate and " });
        }
        println!("Done.  [dry-run] No changes made.");
        return Ok(());
    }

    if release {
        app_store_release(ctx, app, &p, creds.unwrap())
    } else {
        testflight(ctx, app, &p, build_only, creds)
    }
}

fn testflight(ctx: &Ctx, app: &AppStoreCfg, p: &Paths, build_only: bool, creds: Option<Credentials>) -> Result<()> {
    let dir = ctx.root.join(&app.build_dir);
    let archive = dir.join(format!("{}.xcarchive", p.app_name));
    let export = dir.join("export");
    println!("\nArchiving {} {} for iOS…", p.app_name, ctx.version);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    // generic/platform=iOS produces a universal arm64 device archive;
    // -allowProvisioningUpdates lets Xcode fetch the App Store profile.
    xcodebuild(
        &[
            "archive", "-project", s(&p.project)?, "-scheme", &p.scheme, "-configuration", "Release",
            "-destination", "generic/platform=iOS", "-archivePath", s(&archive)?, "-allowProvisioningUpdates",
        ],
        &["Build", "Archive"],
    )?;
    println!("Exporting IPA…");
    xcodebuild(
        &[
            "-exportArchive", "-archivePath", s(&archive)?, "-exportPath", s(&export)?,
            "-exportOptionsPlist", s(&app.export_options)?, "-allowProvisioningUpdates",
        ],
        &["Export"],
    )?;
    // Xcode 15+ puts the IPA in Apps/; older versions directly in the export dir.
    let ipa = [export.join("Apps").join(format!("{}.ipa", p.app_name)), export.join(format!("{}.ipa", p.app_name))]
        .into_iter()
        .find(|f| f.is_file());
    let Some(ipa) = ipa else {
        let _ = sh::run("find", &[s(&export)?, "-maxdepth", "3"]);
        bail!("IPA not found after export");
    };
    println!("IPA: {}", ipa.display());
    if build_only {
        println!("\nDone.  Build-only: IPA at {}", ipa.display());
        return Ok(());
    }

    let creds = creds.context("credentials")?;
    let altool = |action: &str| {
        sh::run(
            "xcrun",
            &[
                "altool", action, "-f", s(&ipa)?, "--type", "ios", "--apiKey", &creds.key_id,
                "--apiIssuer", &creds.issuer_id, "--output-format", "normal",
            ],
        )
    };
    // --validate-app checks signing, entitlements and bundle ID before
    // consuming an upload slot.
    if std::env::var("SKIP_VALIDATE").as_deref() == Ok("1") {
        println!("Skipping validation (SKIP_VALIDATE=1).");
    } else {
        println!("Validating IPA against App Store Connect…");
        altool("--validate-app")?;
        println!("Validation passed.");
    }
    // altool is deprecated in favour of Transporter / the API but still
    // works; its deprecation warning is expected.
    println!("Uploading to App Store Connect (this may take a minute)…");
    altool("--upload-app")?;

    println!("\n══ Upload complete: {} {} ══\n", p.app_name, ctx.version);
    println!("Next steps:");
    println!("  1. Wait ~5–15 minutes for Apple to process the build (App Store Connect → TestFlight).");
    println!("  2. Make sure these are current and committed/pushed in the '{}' worktree:", app.worktree.display());
    println!("       - screenshots/  (iphone-NN*.png / ipad-NN*.png)");
    println!("       - metadata/description.txt, keywords.txt, …");
    println!("     (whatsnew.txt was already generated and committed by `release`.)");
    println!("  3. When ready: release ios --release");
    println!("\nTo re-upload a binary for {}, increment CURRENT_PROJECT_VERSION first.", ctx.version);
    Ok(())
}

fn app_store_release(ctx: &Ctx, app: &AppStoreCfg, p: &Paths, creds: Credentials) -> Result<()> {
    review_metadata(p)?;
    let screenshots = review_screenshots(p, app.screenshot_max_age_days)?;
    let whatsnew = read_nonempty(&p.whatsnew)
        .with_context(|| format!("{} not found or empty. Run `release` first to generate and commit it.", p.whatsnew.display()))?;
    ctx::section("What's New (whatsnew.txt)");
    println!("{whatsnew}");
    ctx::end_section();
    ctx.require_confirm("App metadata, release notes and screenshots updated and committed?")?;

    // Work against the build a previous TestFlight run uploaded, and make
    // sure it's the one matching the local project.
    let asc = Asc::new(creds)?;
    ctx::section("Verifying uploaded build");
    println!("Looking up app ID for {}…", app.bundle_id);
    let app_id = asc.app_id(&app.bundle_id)?;
    println!("Looking for processed build {} in App Store Connect…", ctx.version);
    let build_id = asc.find_build(&app_id, &ctx.version)?.with_context(|| {
        format!("no processed build for {} in App Store Connect.\n  Upload a TestFlight build first: release ios", ctx.version)
    })?;
    let remote_build = asc.build_number(&build_id)?;
    let local_build = local_build_number(ctx, &p.project)?;
    if remote_build != local_build {
        bail!(
            "build number mismatch.\n  App Store Connect: {v} ({remote_build})\n  Local project:     {v} ({local_build})\n  \
             If you re-uploaded with a new build number, update CURRENT_PROJECT_VERSION to match, then re-run.",
            v = ctx.version
        );
    }
    println!("  Build {} ({remote_build}) verified — matches the local project.", ctx.version);
    ctx::end_section();

    ctx::section("App Store Connect");
    println!("Checking for App Store version {}…", ctx.version);
    let version_id = match asc.find_version(&app_id, &ctx.version)? {
        Some(id) => {
            println!("  Found existing version ID: {id}");
            id
        }
        None => {
            println!("Creating App Store version {}…", ctx.version);
            let id = asc.create_version(&app_id, &ctx.version)?;
            println!("  Created version ID: {id}");
            id
        }
    };
    println!("Fetching {} localization…", app.locale);
    let loc_id = asc.localization(&version_id, &app.locale)?;

    println!("Uploading metadata…");
    let mut attrs = vec![
        ("whatsNew", whatsnew),
        ("description", read_nonempty(&p.metadata.join("description.txt")).context("description.txt")?),
    ];
    for (file, attr) in [("keywords.txt", "keywords"), ("support_url.txt", "supportUrl"), ("marketing_url.txt", "marketingUrl")] {
        if let Some(v) = read_nonempty(&p.metadata.join(file)) {
            attrs.push((attr, v));
        }
    }
    asc.update_localization(&loc_id, &attrs)?;

    println!("Associating build {build_id} with version {}…", ctx.version);
    asc.associate_build(&version_id, &build_id)?;

    for (display_type, files) in &screenshots {
        println!("Uploading {} {display_type} screenshot(s)…", files.len());
        asc.replace_screenshots(&loc_id, display_type, files)?;
    }
    ctx::end_section();

    println!("App Store Connect steps completed:");
    println!("  ✓ Version {} created / found", ctx.version);
    println!("  ✓ Metadata and release notes uploaded");
    println!("  ✓ Build associated");
    println!("  ✓ Screenshots uploaded");
    println!("\nRemaining manual step:");
    println!("  App Store Connect → Apps → {} → iOS → {}: review all fields, then submit for review.", p.app_name, ctx.version);
    Ok(())
}

fn review_metadata(p: &Paths) -> Result<()> {
    let mut errors = Vec::new();
    for f in ["description.txt", "support_url.txt", "privacy_url.txt"] {
        let path = p.metadata.join(f);
        match std::fs::read_to_string(&path) {
            Ok(t) if t.trim().is_empty() => errors.push(format!("{} is empty", path.display())),
            Ok(t) if t.lines().any(|l| l.starts_with("TODO:")) => {
                errors.push(format!("{} still contains a TODO placeholder", path.display()))
            }
            Ok(_) => {}
            Err(_) => errors.push(format!("{} is missing", path.display())),
        }
    }
    for f in ["support_url.txt", "privacy_url.txt", "marketing_url.txt"] {
        let path = p.metadata.join(f);
        if let Some(url) = read_nonempty(&path)
            && !url.starts_with("TODO:")
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            errors.push(format!("{} does not look like a URL: {url}", path.display()));
        }
    }
    if !errors.is_empty() {
        bail!("App Store metadata problems:\n  {}", errors.join("\n  "));
    }
    Ok(())
}

/// List all screenshots with their age, check the uploadable sets' sizes,
/// and return them grouped by display type.
fn review_screenshots(p: &Paths, max_age_days: u64) -> Result<Vec<(&'static str, Vec<PathBuf>)>> {
    let mut all: Vec<PathBuf> = std::fs::read_dir(&p.screenshots)
        .with_context(|| format!("screenshots directory not found: {}", p.screenshots.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|f| {
            f.is_file()
                && f.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| ["png", "jpg", "jpeg"].contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect();
    all.sort();
    if all.is_empty() {
        bail!("no screenshots found in {}", p.screenshots.display());
    }
    ctx::section("Screenshots");
    let now = SystemTime::now();
    for f in &all {
        let modified = std::fs::metadata(f)?.modified()?;
        let age = now.duration_since(modified).unwrap_or_default().as_secs() / 86400;
        let date = chrono::DateTime::<chrono::Local>::from(modified).format("%Y-%m-%d");
        let tag = if age > max_age_days { "STALE" } else { "ok" };
        println!("  [{tag:<6} {age}d]  {date}  {}", name(f));
    }
    ctx::end_section();

    let mut groups = Vec::new();
    for &(prefix, display_type, (w, h)) in SCREENSHOT_SETS {
        let files: Vec<PathBuf> = all
            .iter()
            .filter(|f| name(f).starts_with(prefix) && name(f).ends_with(".png"))
            .cloned()
            .collect();
        for f in &files {
            let size = imagesize::size(f).with_context(|| format!("cannot read image size of {}", f.display()))?;
            let (fw, fh) = (size.width as u32, size.height as u32);
            if (fw, fh) != (w, h) && (fh, fw) != (w, h) {
                bail!("{} is {fw}×{fh} px; expected {w}×{h} for {display_type}", name(f));
            }
        }
        if !files.is_empty() {
            groups.push((display_type, files));
        }
    }
    if groups.is_empty() {
        bail!(
            "no iphone-*.png / ipad-*.png screenshots in {}; rename them to match the convention",
            p.screenshots.display()
        );
    }
    Ok(groups)
}

fn local_build_number(ctx: &Ctx, project: &Path) -> Result<String> {
    let pbx = ctx.root.join(project).join("project.pbxproj");
    let text = std::fs::read_to_string(&pbx).with_context(|| format!("cannot read {}", pbx.display()))?;
    let re = regex::Regex::new(r"CURRENT_PROJECT_VERSION = ([^;\s]+);").unwrap();
    re.captures(&text)
        .map(|c| c[1].to_string())
        .with_context(|| format!("no CURRENT_PROJECT_VERSION in {}", pbx.display()))
}

fn read_nonempty(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn name(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

fn s(p: &Path) -> Result<&str> {
    p.to_str().context("non-UTF-8 path")
}
