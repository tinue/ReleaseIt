//! `renew-certs` end to end: a fake `gh` that keeps secrets and variables in
//! files, a throwaway keychain, and test certificates from tests/fixtures/certs
//! (issued by fake "G2" and old-style CAs for team TESTTEAM01).

use base64::Engine;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

const FAKE_GH: &str = r#"#!/bin/bash
# State: $GH_STATE/<repo with / as _>.<NAME>; log: $GH_STATE/log.
echo "$*" >> "$GH_STATE/log"
f="$GH_STATE/${5//\//_}.$3"
case "$1 $2" in
  "secret set") cat > "$f.secret" ;;
  "variable set") printf %s "$7" > "$f.var" ;;
  "variable get") [ -f "$f.var" ] || exit 1; cat "$f.var" ;;
  *) echo "fake gh: unsupported: $*" >&2; exit 2 ;;
esac
"#;

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/certs");
const REPOS: [&str; 2] = ["me/One", "me/Two"];

struct Env {
    tmp: tempfile::TempDir,
    state: PathBuf,
    keychain: PathBuf,
    path: String,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let state = tmp.path().join("gh");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&state).unwrap();
        let gh = bin.join("gh");
        fs::write(&gh, FAKE_GH).unwrap();
        Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
        fs::write(tmp.path().join("certs.yaml"), format!("team_id: TESTTEAM01\nrepos: [{}]\n", REPOS.join(", "))).unwrap();
        let keychain = tmp.path().join("test.keychain-db");
        let kc = keychain.to_str().unwrap();
        security(&["create-keychain", "-p", "kc", kc]);
        security(&["unlock-keychain", "-p", "kc", kc]);
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        Env { tmp, state, keychain, path }
    }

    /// A copy of a fixture .p12 (apply deletes its inputs).
    fn p12(&self, name: &str) -> PathBuf {
        let p = self.tmp.path().join(format!("{name}.p12"));
        fs::copy(format!("{FIX}/{name}.p12"), &p).unwrap();
        p
    }

    fn run(&self, args: &[&str]) -> Output {
        let out = Command::new(env!("CARGO_BIN_EXE_renew-certs"))
            .args(["--config", self.tmp.path().join("certs.yaml").to_str().unwrap()])
            .args(["--keychain", self.keychain.to_str().unwrap()])
            .args(args)
            .env("PATH", &self.path)
            .env("GH_STATE", &self.state)
            .env_remove("RENEW_CERTS_P12_PASSWORD")
            .output()
            .unwrap();
        eprintln!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        out
    }

    fn state(&self, repo: &str, name: &str, ext: &str) -> Option<Vec<u8>> {
        fs::read(self.state.join(format!("{}.{name}.{ext}", repo.replace('/', "_")))).ok()
    }

    fn set_var(&self, repo: &str, name: &str, value: &str) {
        fs::write(self.state.join(format!("{}.{name}.var", repo.replace('/', "_"))), value).unwrap();
    }

    fn log(&self) -> String {
        fs::read_to_string(self.state.join("log")).unwrap_or_default()
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
fn apply_sets_secrets_and_variables_in_every_repo() {
    let env = Env::new();
    let (app, inst) = (env.p12("application-g2"), env.p12("installer-g2"));
    let out = env.run(&[
        "apply", "--yes", "--password", "test",
        "--application", app.to_str().unwrap(), "--installer", inst.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    for repo in REPOS {
        let b64 = env.state(repo, "APPLE_CERT_APPLICATION_P12_BASE64", "secret").unwrap();
        let p12 = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
        assert_eq!(p12, fs::read(format!("{FIX}/application-g2.p12")).unwrap());
        assert!(env.state(repo, "APPLE_CERT_INSTALLER_P12_BASE64", "secret").is_some());
        assert_eq!(env.state(repo, "APPLE_CERT_P12_PASSWORD", "secret").unwrap(), b"test");
        assert_eq!(env.state(repo, "APPLE_CERT_APPLICATION_SHA1", "var").unwrap().len(), 40);
        assert!(env.state(repo, "APPLE_CERT_INSTALLER_EXPIRES", "var").unwrap().starts_with(b"2046-"));
    }
    assert!(!env.log().contains("test"), "password on a gh command line:\n{}", env.log());
    assert!(!app.exists() && !inst.exists(), "inputs not deleted");
    assert_eq!(env.identities().matches("SHA-1 hash").count(), 2, "not imported");

    let out = env.run(&["status"]);
    assert_eq!(out.status.code(), Some(0), "status after apply");
    assert!(stdout(&out).contains("← CI: me/One, me/Two"));
}

#[test]
fn dry_run_changes_nothing() {
    let env = Env::new();
    let app = env.p12("application-g2");
    let out = env.run(&["--dry-run", "apply", "--password", "test", "--application", app.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("[dry-run] gh secret set APPLE_CERT_APPLICATION_P12_BASE64 -R me/One"));
    assert!(env.state("me/One", "APPLE_CERT_APPLICATION_P12_BASE64", "secret").is_none());
    assert!(!env.log().contains("variable set"));
    assert!(app.exists());
}

#[test]
fn one_certificate_leaves_the_password_alone() {
    let env = Env::new();
    let inst = env.p12("installer-g2");
    let out = env.run(&["apply", "-y", "--password", "test", "--installer", inst.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(env.state("me/One", "APPLE_CERT_INSTALLER_P12_BASE64", "secret").is_some());
    assert!(env.state("me/One", "APPLE_CERT_P12_PASSWORD", "secret").is_none());
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
    assert!(env.state("me/One", "APPLE_CERT_INSTALLER_P12_BASE64", "secret").is_none());
    assert!(old.exists() && app.exists());
}

#[test]
fn refuses_a_downgrade_unless_forced() {
    let env = Env::new();
    env.set_var("me/Two", "APPLE_CERT_APPLICATION_SHA1", "AB");
    env.set_var("me/Two", "APPLE_CERT_APPLICATION_EXPIRES", "2099-01-01T00:00:00Z");
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
