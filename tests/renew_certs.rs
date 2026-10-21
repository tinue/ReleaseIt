//! `renew-certs` end to end: a git repo with a release.yaml `certs:` section,
//! a fake `gh` that keeps secrets and variables in files, a fake `op` that
//! keeps 1Password items in directories, and a throwaway keychain holding
//! test identities from tests/fixtures/certs (issued by fake "G2" and
//! old-style CAs for team TESTTEAM01).

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

/// Items are directories $OP_STATE/<id>/ with title, password, vault, tags
/// and (once attached) file; every call goes to $OP_STATE/log.
const FAKE_OP: &str = r#"#!/bin/bash
echo "$*" >> "$OP_STATE/log"
item() {  # JSON of item $1
  d="$OP_STATE/$1"; files='[]'; [ -f "$d/file" ] && files='[{"id":"f1","name":"x.p12"}]'
  printf '{"id":"%s","title":"%s","fields":[{"id":"password","value":"%s"}],"files":%s}' \
    "$1" "$(cat "$d/title")" "$(cat "$d/password")" "$files"
}
case "$1 $2" in
  "item list")
    sep=; printf '['
    for d in "$OP_STATE"/*/; do
      [ -f "$d/title" ] || continue; id=$(basename "$d")
      printf '%s{"id":"%s","title":"%s"}' "$sep" "$id" "$(cat "$d/title")"; sep=,
    done
    printf ']' ;;
  "item create")
    id=item$(ls -d "$OP_STATE"/item* 2>/dev/null | wc -l | tr -d ' '); d="$OP_STATE/$id"; mkdir "$d"
    shift 2
    while [ $# -gt 0 ]; do
      case "$1" in
        --title) printf %s "$2" > "$d/title"; shift ;;
        --vault) printf %s "$2" > "$d/vault"; shift ;;
        --tags) printf %s "$2" > "$d/tags"; shift ;;
        notesPlain=*) printf %s "${1#notesPlain=}" > "$d/notes" ;;
      esac; shift
    done
    printf %s "pw-$id-generated" > "$d/password"; item "$id" ;;
  "item get") item "$3" ;;
  "item edit") cp "${6#*\[file\]=}" "$OP_STATE/$3/file" ;;
  *) echo "fake op: unsupported: $*" >&2; exit 2 ;;
esac
"#;

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/certs");

struct Env {
    _tmp: tempfile::TempDir,
    work: PathBuf,
    gh: FakeGh,
    op_state: PathBuf,
    keychain: PathBuf,
}

impl Env {
    /// A repo and a keychain with the identities from these fixture .p12s.
    fn new(identities: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let gh = FakeGh::install(tmp.path(), FAKE_GH);
        let op = tmp.path().join("bin/op");
        fs::write(&op, FAKE_OP).unwrap();
        assert!(Command::new("chmod").arg("+x").arg(&op).status().unwrap().success());
        let op_state = tmp.path().join("op");
        fs::create_dir_all(&op_state).unwrap();
        let work = tmp.path().join("work");
        fs::create_dir_all(work.join("sub")).unwrap();
        assert!(Command::new("git").arg("init").arg("-q").arg(&work).status().unwrap().success());
        fs::write(work.join("release.yaml"), "name: Demo\nversion: { preset: cargo }\ncerts:\n  team_id: TESTTEAM01\n")
            .unwrap();
        let keychain = tmp.path().join("test.keychain-db");
        let kc = keychain.to_str().unwrap();
        security(&["create-keychain", "-p", "kc", kc]);
        security(&["unlock-keychain", "-p", "kc", kc]);
        for name in identities {
            // -A: any app may use the key, so exporting it shows no dialog.
            security(&["import", &format!("{FIX}/{name}.p12"), "-k", kc, "-P", "test", "-A"]);
        }
        Env { _tmp: tmp, work, gh, op_state, keychain }
    }

    fn run(&self, args: &[&str]) -> Output {
        // From a subdirectory: the tool finds the repo root like `release`.
        self.gh.run(
            Command::new(env!("CARGO_BIN_EXE_renew-certs"))
                .current_dir(self.work.join("sub"))
                .args(["--keychain", self.keychain.to_str().unwrap()])
                .args(args)
                .env("OP_STATE", &self.op_state),
        )
    }

    fn state(&self, name: &str, ext: &str) -> Option<Vec<u8>> {
        fs::read(self.gh.state.join(format!("{name}.{ext}"))).ok()
    }

    fn set_var(&self, name: &str, value: &str) {
        fs::write(self.gh.state.join(format!("{name}.var")), value).unwrap();
    }

    fn identities(&self) -> usize {
        let out = Command::new("security")
            .args(["find-certificate", "-a", "-Z", self.keychain.to_str().unwrap()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).matches("SHA-1 hash").count()
    }

    /// (title, password, attached file) of each fake 1Password item.
    fn op_items(&self) -> Vec<(String, String, Option<Vec<u8>>)> {
        let mut items: Vec<_> = fs::read_dir(&self.op_state)
            .unwrap()
            .filter_map(|e| {
                let d = e.unwrap().path();
                let title = fs::read_to_string(d.join("title")).ok()?;
                Some((title, fs::read_to_string(d.join("password")).unwrap(), fs::read(d.join("file")).ok()))
            })
            .collect();
        items.sort();
        items
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

/// The certificate subject in a .p12, opened with `password`.
fn p12_subject(p12: &[u8], password: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("x.p12");
    fs::write(&f, p12).unwrap();
    let out = Command::new("/usr/bin/openssl")
        .args(["pkcs12", "-in", f.to_str().unwrap(), "-passin", "env:PW", "-nokeys", "-clcerts"])
        .env("PW", password)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const BOTH: &[&str] = &["application-g2", "installer-g2"];

#[test]
fn apply_sets_both_secrets_under_a_random_password() {
    let env = Env::new(BOTH);
    let out = env.run(&["apply", "--yes"]);
    assert!(out.status.success());
    let pw = String::from_utf8(env.state("APPLE_CERT_P12_PASSWORD", "secret").unwrap()).unwrap();
    assert_eq!(pw.len(), 40);
    for (secret, cn) in [
        ("APPLE_CERT_APPLICATION_P12_BASE64", "Developer ID Application"),
        ("APPLE_CERT_INSTALLER_P12_BASE64", "Developer ID Installer"),
    ] {
        let p12 = base64::engine::general_purpose::STANDARD.decode(env.state(secret, "secret").unwrap()).unwrap();
        assert!(p12_subject(&p12, &pw).contains(cn), "{secret}");
    }
    assert_eq!(env.state("APPLE_CERT_APPLICATION_SHA1", "var").unwrap().len(), 40);
    assert!(env.state("APPLE_CERT_INSTALLER_EXPIRES", "var").unwrap().starts_with(b"2046-"));
    assert!(!env.gh.log().contains(&pw), "password on a gh command line");
    assert!(stdout(&out).contains("renew-certs backup"));

    // A second run gets a new password.
    assert!(env.run(&["apply", "--yes"]).status.success());
    assert_ne!(env.state("APPLE_CERT_P12_PASSWORD", "secret").unwrap(), pw.as_bytes());

    let out = env.run(&["status"]);
    assert_eq!(out.status.code(), Some(0), "status after apply");
    assert_eq!(stdout(&out).matches("← CI").count(), 2);
}

#[test]
fn dry_run_changes_nothing() {
    let env = Env::new(BOTH);
    let out = env.run(&["--dry-run", "apply"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("[dry-run] gh secret set APPLE_CERT_APPLICATION_P12_BASE64 "));
    assert!(env.state("APPLE_CERT_APPLICATION_P12_BASE64", "secret").is_none());
    assert!(!env.gh.log().contains("variable set"));
}

#[test]
fn needs_both_g2_identities() {
    let env = Env::new(&["application-g2", "installer-oldca"]);
    let out = env.run(&["apply", "-y"]);
    assert!(!out.status.success());
    assert!(stdout(&out).contains("no G2 Developer ID Installer identity"));
    assert!(env.state("APPLE_CERT_APPLICATION_P12_BASE64", "secret").is_none());
}

#[test]
fn refuses_a_downgrade_unless_forced() {
    let env = Env::new(BOTH);
    env.set_var("APPLE_CERT_APPLICATION_SHA1", "AB");
    env.set_var("APPLE_CERT_APPLICATION_EXPIRES", "2099-01-01T00:00:00Z");
    let out = env.run(&["apply", "-y"]);
    assert!(!out.status.success());
    assert!(stdout(&out).contains("--force to downgrade"));
    assert!(env.run(&["apply", "-y", "--force"]).status.success());
}

#[test]
fn status_flags_unrecorded_ci_and_missing_backups() {
    let env = Env::new(BOTH);
    let out = env.run(&["status"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("unknown (not recorded by renew-certs)"));
    assert_eq!(stdout(&out).matches("NO BACKUP").count(), 2);
}

#[test]
fn cleanup_deletes_the_superseded_old_ca_identity() {
    let env = Env::new(&["application-g2", "installer-g2", "installer-oldca"]);
    assert_eq!(env.identities(), 3);
    let out = env.run(&["apply", "-y"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("Old identities"));
    assert_eq!(env.identities(), 2);
}

#[test]
fn backup_stores_each_identity_once() {
    let env = Env::new(BOTH);
    let out = env.run(&["backup", "-y"]);
    assert!(out.status.success());
    let items = env.op_items();
    assert_eq!(items.len(), 2);
    for (title, pw, file) in &items {
        assert!(title.starts_with("developer-id-application-2046-") || title.starts_with("developer-id-installer-2046-"));
        let p12 = file.as_ref().expect("attachment");
        assert!(p12_subject(p12, pw).contains("TESTTEAM01"), "{title}");
    }
    let log = fs::read_to_string(env.op_state.join("log")).unwrap();
    assert!(log.contains("--vault Private --title developer-id-"));
    assert!(log.contains("--tags renew-certs"));
    assert!(!items.iter().any(|(_, pw, _)| log.contains(pw.as_str())), "password on an op command line");

    // Again: nothing new.
    let out = env.run(&["backup", "-y"]);
    assert_eq!(stdout(&out).matches("already backed up").count(), 2);
    assert_eq!(env.op_items().len(), 2);
    let status = stdout(&env.run(&["status"]));
    assert_eq!(status.matches("  backed up").count(), 2, "{status}");
}

#[test]
fn backup_completes_an_item_without_attachment() {
    let env = Env::new(BOTH);
    assert!(env.run(&["backup", "-y"]).status.success());
    let first = fs::read_dir(&env.op_state).unwrap().map(|e| e.unwrap().path()).find(|d| d.join("file").exists());
    fs::remove_file(first.unwrap().join("file")).unwrap();
    let out = env.run(&["backup", "-y"]);
    assert_eq!(stdout(&out).matches("already backed up").count(), 1);
    assert_eq!(env.op_items().len(), 2);
    assert!(env.op_items().iter().all(|(_, _, f)| f.is_some()));
}

#[test]
fn needs_a_certs_section() {
    let env = Env::new(&[]);
    fs::write(env.work.join("release.yaml"), "name: Demo\nversion: { preset: cargo }\n").unwrap();
    let out = env.run(&["status"]);
    assert!(!out.status.success());
    assert!(stdout(&out).contains("no `certs:` section in release.yaml"));
}
