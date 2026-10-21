//! `renew-certs` end to end: a git repo with a release.yaml `certs:` section,
//! a fake `gh` that keeps secrets and variables in files, a throwaway
//! keychain, and test certificates from tests/fixtures/certs (issued by fake
//! "G2" and old-style CAs for team TESTTEAM01).

mod common;

use base64::Engine;
use common::FakeGh;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const FAKE_GH: &str = r#"#!/bin/bash
# State: $GH_STATE/<NAME>.secret|var; log: $GH_STATE/log.
echo "$*" >> "$GH_STATE/log"
f="$GH_STATE/$3"
case "$1 $2" in
  "secret set") cat > "$f.secret" ;;
  "variable set") printf %s "$5" > "$f.var" ;;
  "variable list")
    sep=; printf '['
    for v in "$GH_STATE"/*.var; do
      [ -e "$v" ] || continue
      printf '%s{"name":"%s","value":"%s"}' "$sep" "$(basename "$v" .var)" "$(cat "$v")"; sep=,
    done
    printf ']' ;;
  *) echo "fake gh: unsupported: $*" >&2; exit 2 ;;
esac
"#;

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/certs");

struct Env {
    tmp: tempfile::TempDir,
    work: PathBuf,
    gh: FakeGh,
    keychain: PathBuf,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let gh = FakeGh::install(tmp.path(), FAKE_GH);
        let work = tmp.path().join("work");
        fs::create_dir_all(work.join("sub")).unwrap();
        assert!(Command::new("git").arg("init").arg("-q").arg(&work).status().unwrap().success());
        fs::write(work.join("release.yaml"), "name: Demo\nversion: { preset: cargo }\ncerts:\n  team_id: TESTTEAM01\n")
            .unwrap();
        let keychain = tmp.path().join("test.keychain-db");
        let kc = keychain.to_str().unwrap();
        security(&["create-keychain", "-p", "kc", kc]);
        security(&["unlock-keychain", "-p", "kc", kc]);
        Env { tmp, work, gh, keychain }
    }

    /// A copy of a fixture .p12 (apply deletes its inputs).
    fn p12(&self, name: &str) -> PathBuf {
        let p = self.tmp.path().join(format!("{name}.p12"));
        fs::copy(format!("{FIX}/{name}.p12"), &p).unwrap();
        p
    }

    fn run(&self, args: &[&str]) -> Output {
        // From a subdirectory: the tool finds the repo root like `release`.
        self.gh.run(
            Command::new(env!("CARGO_BIN_EXE_renew-certs"))
                .current_dir(self.work.join("sub"))
                .args(["--keychain", self.keychain.to_str().unwrap()])
                .args(args)
                .env_remove("RENEW_CERTS_P12_PASSWORD"),
        )
    }

    fn state(&self, name: &str, ext: &str) -> Option<Vec<u8>> {
        fs::read(self.gh.state.join(format!("{name}.{ext}"))).ok()
    }

    fn set_var(&self, name: &str, value: &str) {
        fs::write(self.gh.state.join(format!("{name}.var")), value).unwrap();
    }

    fn log(&self) -> String {
        self.gh.log()
    }

    fn identities(&self) -> String {
        let out = Command::new("security")
            .args(["find-certificate", "-a", "-Z", self.keychain.to_str().unwrap()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        security(&["delete-keychain", self.keychain.to_str().unwrap()]);
    }
}

fn security(args: &[&str]) {
    let out = Command::new("security").args(args).output().unwrap();
    assert!(out.status.success(), "security {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn stdout(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[test]
fn apply_sets_secrets_and_variables() {
    let env = Env::new();
    let (app, inst) = (env.p12("application-g2"), env.p12("installer-g2"));
    let out = env.run(&[
        "apply", "--yes", "--password", "test",
        "--application", app.to_str().unwrap(), "--installer", inst.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    let b64 = env.state("APPLE_CERT_APPLICATION_P12_BASE64", "secret").unwrap();
    let p12 = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
    assert_eq!(p12, fs::read(format!("{FIX}/application-g2.p12")).unwrap());
    assert!(env.state("APPLE_CERT_INSTALLER_P12_BASE64", "secret").is_some());
    assert_eq!(env.state("APPLE_CERT_P12_PASSWORD", "secret").unwrap(), b"test");
    assert_eq!(env.state("APPLE_CERT_APPLICATION_SHA1", "var").unwrap().len(), 40);
    assert!(env.state("APPLE_CERT_INSTALLER_EXPIRES", "var").unwrap().starts_with(b"2046-"));
    assert!(!env.log().contains("test"), "password on a gh command line:\n{}", env.log());
    assert!(!app.exists() && !inst.exists(), "inputs not deleted");
    assert_eq!(env.identities().matches("SHA-1 hash").count(), 2, "not imported");

    let out = env.run(&["status"]);
    assert_eq!(out.status.code(), Some(0), "status after apply");
    assert_eq!(stdout(&out).matches("← CI").count(), 2);
}

#[test]
fn dry_run_changes_nothing() {
    let env = Env::new();
    let app = env.p12("application-g2");
    let out = env.run(&["--dry-run", "apply", "--password", "test", "--application", app.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("[dry-run] gh secret set APPLE_CERT_APPLICATION_P12_BASE64 "));
    assert!(env.state("APPLE_CERT_APPLICATION_P12_BASE64", "secret").is_none());
    assert!(!env.log().contains("variable set"));
    assert!(app.exists());
}

#[test]
fn one_certificate_leaves_the_password_alone() {
    let env = Env::new();
    let inst = env.p12("installer-g2");
    let out = env.run(&["apply", "-y", "--password", "test", "--installer", inst.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(env.state("APPLE_CERT_INSTALLER_P12_BASE64", "secret").is_some());
    assert!(env.state("APPLE_CERT_P12_PASSWORD", "secret").is_none());
    assert!(stdout(&out).contains("must use the password CI already has"));
}

#[test]
fn rejects_old_ca_and_wrong_password() {
    let env = Env::new();
    let old = env.p12("installer-oldca");
    let out = env.run(&["apply", "-y", "--password", "test", "--installer", old.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(stdout(&out).contains("original Developer ID CA"));

    let app = env.p12("application-g2");
    let out = env.run(&["apply", "-y", "--password", "wrong", "--application", app.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(stdout(&out).contains("wrong password"));
    assert!(env.state("APPLE_CERT_INSTALLER_P12_BASE64", "secret").is_none());
    assert!(old.exists() && app.exists());
}

#[test]
fn refuses_a_downgrade_unless_forced() {
    let env = Env::new();
    env.set_var("APPLE_CERT_APPLICATION_SHA1", "AB");
    env.set_var("APPLE_CERT_APPLICATION_EXPIRES", "2099-01-01T00:00:00Z");
    let app = env.p12("application-g2");
    let args = ["apply", "-y", "--password", "test", "--application", app.to_str().unwrap()];
    let out = env.run(&args);
    assert!(!out.status.success());
    assert!(stdout(&out).contains("--force to downgrade"));
    let out = env.run(&[&args[..], &["--force"]].concat());
    assert!(out.status.success());
}

#[test]
fn status_flags_unrecorded_ci() {
    let env = Env::new();
    let out = env.run(&["status"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("unknown (not recorded by renew-certs)"));
}

#[test]
fn cleanup_deletes_the_superseded_old_ca_identity() {
    let env = Env::new();
    let kc = env.keychain.to_str().unwrap();
    security(&["import", &format!("{FIX}/installer-oldca.p12"), "-k", kc, "-P", "test"]);
    assert_eq!(env.identities().matches("SHA-1 hash").count(), 1);
    let inst = env.p12("installer-g2");
    let out = env.run(&["apply", "-y", "--password", "test", "--installer", inst.to_str().unwrap()]);
    assert!(out.status.success());
    let ids = env.identities();
    assert_eq!(ids.matches("SHA-1 hash").count(), 1, "{ids}");
    assert!(!ids.contains("oldca") && stdout(&out).contains("Old identities"));
}

#[test]
fn needs_a_certs_section() {
    let env = Env::new();
    fs::write(env.work.join("release.yaml"), "name: Demo\nversion: { preset: cargo }\n").unwrap();
    let out = env.run(&["status"]);
    assert!(!out.status.success());
    assert!(stdout(&out).contains("no `certs:` section in release.yaml"));
}
