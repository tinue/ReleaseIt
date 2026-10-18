//! End-to-end lifecycle against a throwaway repo: a bare "origin", a clone
//! on a dev branch, and a fake `gh` on PATH that keeps release state in files
//! and logs every call.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FAKE_GH: &str = r#"#!/bin/bash
# Minimal gh stand-in. State: $GH_STATE/<tag>.json; log: $GH_STATE/log.
echo "$*" >> "$GH_STATE/log"
case "$1 $2" in
  "release view")
    f="$GH_STATE/$3.json"; [ -f "$f" ] || exit 1; cat "$f" ;;
  "release create")
    tag="$3"; shift 3; n=0
    while [ $# -gt 0 ] && [ "${1#--}" = "$1" ]; do n=$((n+1)); shift; done
    pre=false; for a in "$@"; do [ "$a" = "--prerelease" ] && pre=true; done
    echo "{\"isPrerelease\":$pre,\"isDraft\":false,\"assets\":[$(seq 1 $n | paste -sd, -)],\"url\":\"https://x/$tag\"}" > "$GH_STATE/$tag.json" ;;
  "release edit")
    tag="$3"; shift 3; new="$tag"; pre=""
    while [ $# -gt 0 ]; do
      case "$1" in --tag) new="$2"; shift ;; --prerelease=false) pre=false ;; --prerelease) pre=true ;; esac; shift
    done
    j=$(cat "$GH_STATE/$tag.json")
    [ -n "$pre" ] && j=$(echo "$j" | sed -E "s/\"isPrerelease\":(true|false)/\"isPrerelease\":$pre/")
    echo "$j" | sed "s#https://x/$tag#https://x/$new#" > "$GH_STATE/$new.json"
    [ "$new" != "$tag" ] && rm "$GH_STATE/$tag.json"; true ;;
  "release upload") ;;
  "workflow run") ;;
  "workflow view") if [ -e "$GH_STATE/no-workflow" ]; then exit 1; fi ;;
  "run list")
    echo "[{\"databaseId\":7,\"headSha\":\"old\",\"createdAt\":\"2000-01-01T00:00:00Z\"},{\"databaseId\":42,\"headSha\":\"$(git rev-parse HEAD)\",\"createdAt\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"}]" ;;
  "run watch") [ "$3" = 42 ] ;;
  "run download")
    mkdir -p "$5/app-linux" && echo elf > "$5/app-linux/App-0.1.0.AppImage" ;;
  *) echo "fake gh: unsupported: $*" >&2; exit 2 ;;
esac
"#;

struct Env {
    _tmp: tempfile::TempDir,
    work: PathBuf,
    remote: PathBuf,
    state: PathBuf,
    path: String,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const DEFAULT_YAML: &str = r#"
name: Demo
version: { preset: cargo }
changelog: { wip_marker: WIP }
next: { bump: patch }
release:
  notes_suffix: "**Note**: suffix"
  commit_trailer: "[skip ci]"
build:
  - kind: commands
    run: ["mkdir -p dist && echo bin > dist/demo.bin"]
    assets: ["dist/*.bin"]
"#;

fn setup() -> Env {
    setup_with(DEFAULT_YAML)
}

fn setup_with(yaml: &str) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("origin.git");
    let work = tmp.path().join("work");
    let state = tmp.path().join("gh");
    let bin = tmp.path().join("bin");
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(&gh, FAKE_GH).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();

    git(tmp.path(), &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
    git(tmp.path(), &["clone", "-q", remote.to_str().unwrap(), work.to_str().unwrap()]);
    git(&work, &["config", "user.email", "t@example.com"]);
    git(&work, &["config", "user.name", "Test"]);
    git(&work, &["checkout", "-q", "-b", "main"]);
    fs::write(work.join("README.md"), "x\n").unwrap();
    fs::write(work.join(".gitignore"), "dist/\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "init"]);
    git(&work, &["push", "-q", "-u", "origin", "main"]);

    git(&work, &["checkout", "-q", "-b", "dev-0.1.0"]);
    fs::write(work.join("Cargo.toml"), "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
    fs::write(work.join("CHANGELOG.md"), "# Changelog\n\n## [0.1.0] - WIP\n\n- **Feature** first\n").unwrap();
    fs::write(work.join("release.yaml"), yaml).unwrap();
    fs::create_dir_all(work.join("examples")).unwrap();
    fs::write(work.join("examples/a.txt"), "a\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "dev"]);
    git(&work, &["push", "-q", "-u", "origin", "dev-0.1.0"]);

    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    Env { _tmp: tmp, work, remote, state, path }
}

fn release(env: &Env, args: &[&str]) -> Output {
    let out = Command::new(env!("CARGO_BIN_EXE_release"))
        .current_dir(&env.work)
        .args(args)
        .env("PATH", &env.path)
        .env("GH_STATE", &env.state)
        .output()
        .unwrap();
    eprintln!(
        "--- release {args:?}\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn ok(env: &Env, args: &[&str]) -> String {
    let out = release(env, args);
    assert!(out.status.success(), "release {args:?} failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn gh_log(env: &Env) -> String {
    fs::read_to_string(env.state.join("log")).unwrap_or_default()
}

#[test]
fn full_cycle() {
    let env = setup();

    // Dry-run changes nothing.
    ok(&env, &["--dry-run"]);
    assert_eq!(git(&env.remote, &["tag"]), "");
    assert!(!gh_log(&env).contains("release create"));

    // Pre-release: -pre tag pushed, release created with the built asset.
    ok(&env, &["--yes"]);
    assert_eq!(git(&env.remote, &["tag"]), "v0.1.0-pre");
    let log = gh_log(&env);
    assert!(log.contains("release create v0.1.0-pre"), "{log}");
    assert!(log.contains("dist/demo.bin"));
    assert!(log.contains("--verify-tag"));
    assert!(env.state.join("v0.1.0-pre.json").exists());
    // CHANGELOG untouched by a plain pre-release.
    assert!(fs::read_to_string(env.work.join("CHANGELOG.md")).unwrap().contains("## [0.1.0] - WIP"));

    // Re-cut updates instead of creating.
    ok(&env, &["--yes"]);
    assert!(gh_log(&env).contains("release edit v0.1.0-pre --title"));
    assert!(gh_log(&env).contains("release upload v0.1.0-pre"));

    // --release refuses while the entry is open.
    assert!(!release(&env, &["--yes", "--release"]).status.success());

    // --rc dates the entry, commits and pushes it.
    ok(&env, &["--yes", "--rc"]);
    let cl = fs::read_to_string(env.work.join("CHANGELOG.md")).unwrap();
    assert!(!cl.contains("WIP") && cl.contains("## [0.1.0] - 20"), "{cl}");
    assert_eq!(git(&env.work, &["rev-parse", "HEAD"]), git(&env.remote, &["rev-parse", "dev-0.1.0"]));

    // Unstaged changes get stashed and restored.
    fs::write(env.work.join("README.md"), "dirty\n").unwrap();

    // --release: squash commit on main, real tag, retargeted release, -pre tag gone.
    ok(&env, &["--yes", "--release"]);
    assert_eq!(fs::read_to_string(env.work.join("README.md")).unwrap(), "dirty\n");
    git(&env.work, &["checkout", "-q", "README.md"]);
    assert_eq!(git(&env.work, &["rev-parse", "--abbrev-ref", "HEAD"]), "dev-0.1.0");
    assert_eq!(git(&env.remote, &["tag"]), "v0.1.0");
    let msg = git(&env.remote, &["log", "-1", "--format=%B", "main"]);
    assert!(msg.starts_with("Release v0.1.0\n\n- **Feature** first"), "{msg}");
    assert!(msg.trim_end().ends_with("[skip ci]"));
    assert!(!msg.contains("suffix"), "notes_suffix must not go into the commit");
    assert!(gh_log(&env).contains("release edit v0.1.0-pre --tag v0.1.0"));
    assert!(env.state.join("v0.1.0.json").exists());

    // --next: dev-0.1.1 from main with bumped version and a new section.
    ok(&env, &["--yes", "--next"]);
    assert_eq!(git(&env.work, &["rev-parse", "--abbrev-ref", "HEAD"]), "dev-0.1.1");
    assert!(fs::read_to_string(env.work.join("Cargo.toml")).unwrap().contains("version = \"0.1.1\""));
    let cl = fs::read_to_string(env.work.join("CHANGELOG.md")).unwrap();
    assert!(cl.starts_with("# Changelog\n\n## [0.1.1] - WIP\n\n## [0.1.0] - 20"), "{cl}");
    assert_eq!(git(&env.remote, &["log", "-1", "--format=%s", "dev-0.1.1"]), "Bump version to 0.1.1");

    // A second --next for the same version is refused.
    assert!(!release(&env, &["--yes", "--next", "--version", "0.1.1"]).status.success());
}

#[test]
fn refuses_main_and_unpushed() {
    let env = setup();
    fs::write(env.work.join("x"), "1").unwrap();
    git(&env.work, &["add", "x"]);
    git(&env.work, &["commit", "-q", "-m", "unpushed"]);
    let out = release(&env, &["--yes"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unpushed"));

    // Renamed dev branch still tracking the previous version's remote branch.
    git(&env.work, &["branch", "-m", "dev-0.2.0"]);
    let out = release(&env, &["--yes"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("tracks 'origin/dev-0.1.0' instead of 'origin/dev-0.2.0'"));

    git(&env.work, &["checkout", "-q", "main"]);
    // main predates the release setup; give it the files untracked.
    fs::write(env.work.join("Cargo.toml"), "[package]\nversion = \"0.1.0\"\n").unwrap();
    fs::write(env.work.join("release.yaml"), "name: Demo\nversion: { preset: cargo }\n").unwrap();
    let out = release(&env, &["--yes", "-C", env.work.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("development branch"));
}

#[test]
fn refuses_when_main_has_unmerged_commits() {
    let env = setup();
    git(&env.work, &["checkout", "-q", "main"]);
    fs::write(env.work.join("hotfix"), "1").unwrap();
    git(&env.work, &["add", "hotfix"]);
    git(&env.work, &["commit", "-q", "-m", "hotfix"]);
    git(&env.work, &["push", "-q", "origin", "main"]);
    git(&env.work, &["checkout", "-q", "dev-0.1.0"]);
    let out = release(&env, &["--yes"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("fast-forward"));
}

#[test]
fn ci_dispatch_plus_local_step_publish_together() {
    let env = setup_with(
        r#"
name: Demo
version: { preset: cargo }
changelog: { wip_marker: WIP }
release:
  extra_assets: [{ zip: examples/, as: Demo-examples.zip }]
build:
  - kind: commands
    run: ["mkdir -p dist && echo bin > dist/demo.bin"]
    assets: ["dist/*.bin"]
  - kind: github-dispatch
    workflow: build.yml
    artifacts:
      - { artifact: app-linux, file: "App-*.AppImage", as: Demo-linux.AppImage, executable: true }
"#,
    );
    ok(&env, &["--yes"]);
    let log = gh_log(&env);
    // CI is dispatched before the local step runs, then everything goes up in one create.
    let dispatch = log.find("workflow run build.yml --ref v0.1.0-pre").expect("dispatched");
    let create = log.find("release create v0.1.0-pre").expect("created");
    assert!(dispatch < create);
    let create_line = log.lines().find(|l| l.starts_with("release create")).unwrap();
    for name in ["demo.bin", "Demo-linux.AppImage", "Demo-examples.zip"] {
        assert!(create_line.contains(name), "{name} missing: {create_line}");
    }
    assert!(log.contains("run watch 42"), "picked the run for HEAD, not the old one");
    assert!(env.state.join("v0.1.0-pre.json").exists());
    let json = fs::read_to_string(env.state.join("v0.1.0-pre.json")).unwrap();
    assert!(json.contains("[1,2,3]"), "{json}");
}

#[test]
fn missing_artifact_publishes_nothing() {
    let env = setup_with(
        r#"
name: Demo
version: { preset: cargo }
changelog: { wip_marker: WIP }
build:
  - kind: github-dispatch
    workflow: build.yml
    artifacts:
      - { artifact: app-windows, file: "*.exe", as: Demo.exe }
"#,
    );
    let out = release(&env, &["--yes"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("app-windows"));
    assert!(!gh_log(&env).contains("release create"));
}

#[test]
fn undispatchable_workflow_pushes_nothing() {
    let env = setup_with(
        r#"
name: Demo
version: { preset: cargo }
changelog: { wip_marker: WIP }
build:
  - kind: github-dispatch
    workflow: build.yml
    artifacts: []
"#,
    );
    fs::write(env.state.join("no-workflow"), "").unwrap();
    let out = release(&env, &["--yes"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("isn't on the default branch"));
    assert!(git(&env.remote, &["tag", "-l"]).is_empty(), "no -pre tag pushed");
}

#[test]
fn notes_only_release_without_build_steps() {
    let env = setup_with("name: Demo\nversion: { preset: cargo }\nchangelog: { wip_marker: WIP }\n");
    ok(&env, &["--yes", "--rc"]);
    assert!(gh_log(&env).contains("release create v0.1.0-pre --title"));
    ok(&env, &["--yes", "--release"]);
    assert!(env.state.join("v0.1.0.json").exists());
}
