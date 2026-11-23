//! After `release` / `release --rc` has published: install the build on this
//! Mac, from the published assets themselves — the local file when this run
//! built it, otherwise downloaded from the GitHub release. The .app of every
//! .dmg asset goes into `apps_dir` (exactly the app a later download of the
//! DMG would give), each configured binary into `bin_dir`.

use crate::config::{BinaryInstall, glob_match};
use crate::ctx::Ctx;
use crate::sh;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// One line per install, for the build plan; empty when nothing is installed.
pub fn describe(ctx: &Ctx) -> Vec<String> {
    let i = &ctx.cfg.release.install;
    if !cfg!(target_os = "macos") {
        return vec![];
    }
    let mut out = Vec::new();
    if i.apps && produces_dmg(ctx) {
        out.push(format!("install the DMG's .app into {}", i.apps_dir.display()));
    }
    for b in &i.binaries {
        let name = binary_name(b);
        out.push(format!("install {name} from {} into {}", b.asset, i.bin_dir.display()));
    }
    out
}

/// Whether the build is known to produce a .dmg (used for the plan only; the
/// install itself looks at the actual assets).
fn produces_dmg(ctx: &Ctx) -> bool {
    use crate::config::BuildStep;
    ctx.cfg.build.iter().any(|s| match s {
        BuildStep::XcodeDmg(_) => true,
        BuildStep::GithubDispatch { artifacts, .. } => artifacts.iter().any(|a| is_dmg(&a.name)),
        BuildStep::Commands { assets, .. } => assets.iter().any(|a| is_dmg(a)),
        BuildStep::GithubTagPush { .. } => false,
    })
}

/// `local`: the assets this run built; `published`: the asset names of the
/// GitHub pre-release; `work`: scratch space for downloads and mounts.
pub fn run(ctx: &Ctx, local: &[PathBuf], published: &[String], work: &Path) -> Result<()> {
    let i = &ctx.cfg.release.install;
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let mut names: Vec<String> = local.iter().filter_map(|p| Some(p.file_name()?.to_str()?.to_owned())).collect();
    for n in published {
        if !names.contains(n) {
            names.push(n.clone());
        }
    }
    let downloads = work.join("install");
    let fetch = |name: &str| -> Result<PathBuf> {
        if let Some(p) = local.iter().find(|p| p.file_name().and_then(|f| f.to_str()) == Some(name)) {
            return Ok(p.clone());
        }
        println!("Downloading {name} from {}…", ctx.pre_tag);
        std::fs::create_dir_all(&downloads)?;
        sh::run(
            "gh",
            &["release", "download", &ctx.pre_tag, "--pattern", name, "--dir", sh::path_str(&downloads)?, "--clobber"],
        )?;
        Ok(downloads.join(name))
    };

    // The release is published by now: a failed install is a warning, and
    // the other assets are still installed.
    let mut failed = 0;
    let mut warn = |what: &str, r: Result<()>| {
        if let Err(e) = r {
            eprintln!("Warning: {what} was not installed on this Mac: {e:#}");
            failed += 1;
        }
    };
    if i.apps {
        for name in names.iter().filter(|n| is_dmg(n)) {
            warn(name, (|| install_app(&fetch(name)?, &expand_home(&i.apps_dir)?, work))());
        }
    }
    for b in &i.binaries {
        let matches: Vec<&String> = names.iter().filter(|n| glob_match(&b.asset, n)).collect();
        let r = match matches.as_slice() {
            [name] => (|| install_binary(&fetch(name)?, b, &expand_home(&i.bin_dir)?, work))(),
            _ => Err(anyhow::anyhow!(
                "install.binaries: asset '{}' matched {} assets, expected exactly 1",
                b.asset,
                matches.len()
            )),
        };
        warn(&b.asset, r);
    }
    if failed > 0 {
        eprintln!("Warning: {failed} of this release's apps and binaries are not installed on this Mac (see above).");
    }
    Ok(())
}

fn is_dmg(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".dmg")
}

fn binary_name(b: &BinaryInstall) -> &str {
    let p = b.path.as_deref().unwrap_or(&b.asset);
    p.rsplit('/').next().unwrap_or(p)
}

fn expand_home(p: &Path) -> Result<PathBuf> {
    match p.strip_prefix("~") {
        Ok(rest) => Ok(PathBuf::from(std::env::var("HOME").context("HOME is not set")?).join(rest)),
        Err(_) => Ok(p.to_path_buf()),
    }
}

/// Detaches the mounted DMG however the install ends.
struct Mount(PathBuf);

impl Drop for Mount {
    fn drop(&mut self) {
        if let Ok(m) = sh::path_str(&self.0)
            && !sh::succeeds("hdiutil", &["detach", "-quiet", m])
        {
            let _ = sh::succeeds("hdiutil", &["detach", "-quiet", "-force", m]);
        }
    }
}

fn install_app(dmg: &Path, apps_dir: &Path, work: &Path) -> Result<()> {
    let mnt = tempfile::tempdir_in(work)?;
    sh::run(
        "hdiutil",
        &["attach", "-nobrowse", "-readonly", "-noautoopen", "-quiet", "-mountpoint", sh::path_str(mnt.path())?, sh::path_str(dmg)?],
    )?;
    let mount = Mount(mnt.path().to_path_buf());
    let apps: Vec<PathBuf> = std::fs::read_dir(&mount.0)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "app") && p.is_dir())
        .collect();
    let [app] = apps.as_slice() else {
        bail!("{} holds {} .app bundles, expected exactly 1", dmg.display(), apps.len());
    };
    let name = app.file_name().context("app without name")?.to_string_lossy().into_owned();

    // The app as a downloader gets it: Developer ID signed and notarized.
    sh::run("codesign", &["--verify", "--deep", "--strict", sh::path_str(app)?])?;
    sh::run("spctl", &["--assess", "--type", "execute", sh::path_str(app)?])
        .with_context(|| format!("Gatekeeper rejects {name} (not notarized?); not installed"))?;

    let plist = app.join("Contents/Info.plist");
    let bundle_id = sh::capture("/usr/libexec/PlistBuddy", &["-c", "Print :CFBundleIdentifier", sh::path_str(&plist)?])?;
    let dst = apps_dir.join(&name);
    let tmp = apps_dir.join(format!(".{name}.new"));

    // Replacing a running bundle breaks it: quit it first, start it again after.
    let was_running = is_running(&bundle_id);
    if was_running {
        println!("Quitting {name}…");
        // Qt apps answer the quit event with "user canceled" (-128) and quit
        // right after, so osascript's status says nothing: the loop decides.
        let _ = sh::succeeds("osascript", &["-e", &format!("tell application id \"{bundle_id}\" to quit")]);
        for _ in 0..20 {
            if !is_running(&bundle_id) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if is_running(&bundle_id) {
            bail!("{name} did not quit: {} was not replaced", dst.display());
        }
    }

    // Copied first, so a failed copy leaves the installed app alone.
    let _ = std::fs::remove_dir_all(&tmp);
    sh::run("ditto", &[sh::path_str(app)?, sh::path_str(&tmp)?])?;
    drop(mount);
    if dst.exists() {
        std::fs::remove_dir_all(&dst).with_context(|| format!("cannot remove {}", dst.display()))?;
    }
    std::fs::rename(&tmp, &dst)?;
    println!("Installed {}", dst.display());
    if was_running {
        sh::run("open", &[sh::path_str(&dst)?])?;
    }
    Ok(())
}

fn is_running(bundle_id: &str) -> bool {
    sh::try_capture("osascript", &["-e", &format!("application id \"{bundle_id}\" is running")]).as_deref() == Some("true")
}

fn install_binary(asset: &Path, b: &BinaryInstall, bin_dir: &Path, work: &Path) -> Result<()> {
    let name = binary_name(b);
    let src = match &b.path {
        None => asset.to_path_buf(),
        Some(inner) => {
            let x = tempfile::tempdir_in(work)?.keep();
            let a = sh::path_str(asset)?;
            let lower = a.to_ascii_lowercase();
            if lower.ends_with(".zip") {
                sh::run("ditto", &["-x", "-k", a, sh::path_str(&x)?])?;
            } else if [".tar.gz", ".tgz", ".tar"].iter().any(|e| lower.ends_with(e)) {
                sh::run("tar", &["-xf", a, "-C", sh::path_str(&x)?])?;
            } else {
                bail!("install.binaries: {} is not a .tar.gz, .tgz, .tar or .zip archive", b.asset);
            }
            let p = x.join(inner);
            if !p.is_file() {
                bail!("install.binaries: no '{inner}' in {}", b.asset);
            }
            p
        }
    };
    std::fs::create_dir_all(bin_dir).with_context(|| format!("cannot create {}", bin_dir.display()))?;
    let dst = bin_dir.join(name);
    let tmp = bin_dir.join(format!(".{name}.new"));
    std::fs::copy(&src, &tmp)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    // A rename, not a copy over the old file: a running copy keeps its inode.
    std::fs::rename(&tmp, &dst)?;
    println!("Installed {}", dst.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths() {
        let b = BinaryInstall { asset: "x-macos.tar.gz".into(), path: Some("x-macos/bin/sde".into()) };
        assert_eq!(binary_name(&b), "sde");
        let raw = BinaryInstall { asset: "tool".into(), path: None };
        assert_eq!(binary_name(&raw), "tool");
        assert!(is_dmg("A-1.0.DMG") && !is_dmg("A.AppImage"));
        assert_eq!(expand_home(Path::new("/opt/bin")).unwrap(), Path::new("/opt/bin"));
        assert!(expand_home(Path::new("~/.local/bin")).unwrap().ends_with(".local/bin"));
    }
}
