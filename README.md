# ReleaseIt

One release tool, `release`, for several projects. Each project describes
itself in a `release.yaml` at its repository root; the tool handles the rest of
the release cycle the same way everywhere: GitHub pre-releases, release
candidates, promotion to the final release, and starting the next version.

It replaces the per-project `bin/release` scripts that used to be copied from
project to project and then drifted apart.

> **Why this repository is public:** only because other public repositories
> of mine release through it, and I want everything they depend on to be
> transparently available. It is not meant to be useful for anyone else's
> projects. There is no support: issues, discussions and pull requests are
> not accepted.

## Install

macOS only:

```sh
./install.sh        # cargo build --release, copy release + renew-certs to ~/Applications
```

`~/Applications` must be on `PATH`. Prerequisites depend on the project:
`git` and [`gh`](https://cli.github.com) (`gh auth login`) always; Xcode,
`create-dmg` (`brew install create-dmg`), a Developer ID Application identity
and notarytool credentials for local macOS builds; `zip` for `extra_assets`;
the 1Password CLI `op` (`brew install 1password-cli`, with *Integrate with
1Password CLI* enabled in the app) for `renew-certs backup`.

## The release cycle

```
release                 pre-release: build, publish/update GitHub pre-release vX.Y.Z-pre
release --rc            release candidate: date the CHANGELOG entry, then as above
release --reopen        undo --rc: the CHANGELOG entry is open again
release --release       promote vX.Y.Z-pre to vX.Y.Z in place
release --next          start the next cycle: cut dev-X.Y.Z from main, bump the version
release --build-only    checks + local build steps only (no git, GitHub or notarization)
release ios             iOS TestFlight upload            (projects with an appstore: section)
release ios --release   App Store Connect version setup
```

Global options: `--dry-run` prints every state-changing action instead of
running it, `-y/--yes` answers all prompts with yes, `-C <dir>` runs as if
started there.

1. **`release`**, any number of times while work continues. Runs from a
   development branch that is pushed, has no staged changes and fast-forwards
   into main (unstaged changes are offered to be stashed and are restored at
   the end). The CHANGELOG must have a non-empty `## [X.Y.Z]` section; it stays
   open (`- work in progress`). Pushes the disposable `vX.Y.Z-pre` tag (moved
   with `-f` on every re-cut), runs the build steps and publishes all assets to
   the GitHub pre-release of that tag.
2. **`release --rc`**: the same, but first replaces the open header
   (`## [X.Y.Z] - <wip_marker>` or a bare `## [X.Y.Z]`) with today's date,
   commits and pushes. A date already there is never overwritten.
   **`release --reopen`** undoes that (an accidental or premature `--rc`):
   the dated header goes back to `- <wip_marker>`, committed and pushed. The
   `-pre` tag and pre-release stay; the next `release` re-cuts them. Refused
   once `vX.Y.Z` exists.
3. **`release --release`**, once the release candidate is good. Requires the
   CHANGELOG entry to be dated and the pre-release to exist (with assets, if
   the project has build steps).
   Squash-merges the dev branch onto main as one commit (message: "Release
   vX.Y.Z" plus the CHANGELOG section), tags `vX.Y.Z` there, and retargets the
   pre-release onto that tag **in place** (`gh release edit --tag`): same
   release object, same assets, no rebuild, no re-upload. The `-pre` tag is
   deleted, the result verified.
4. **`release --next [--version X.Y.Z]`**: from main, cut
   `dev-<next>` (default bump per config), bump the version file, run
   `after_bump`, insert a new `## [<next>] - <wip_marker>` section, commit,
   push.

Why a separate `-pre` tag: pre-releases are re-cut many times per version, but
the real tag must be created exactly once, on the release commit on main.
Letting `gh release create` auto-create the real tag on the current commit is
how one project once ended up with a version tag on the previous release's
commit.

Known quirk: since promotion keeps the assets, a binary that embeds a
`git describe` build id still says `vX.Y.Z-pre`.

## `release.yaml`

Only `name` and `version` are required. Full reference with defaults:

```yaml
name: MyApp                      # used in titles, DMG names
main_branch: main

version:
  preset: cargo                  # xcode | cargo | cmake | regex
  file: Cargo.toml               # default per preset (Cargo.toml, CMakeLists.txt); required for xcode/regex
  regex: '...'                   # preset regex only: capture group 1 is the version

changelog:
  file: CHANGELOG.md
  wip_marker: work in progress   # open header: "## [X.Y.Z] - work in progress"
  insert_next: top               # top (after the first line) | after-unreleased
  compare_links: https://github.com/me/repo   # optional, after-unreleased only

next:
  bump: minor                    # major | minor | patch
  branch: dev-{version}
  after_bump: []                 # shell commands, e.g. ["cargo check --quiet"]
  extra_files: []                # committed with the bump, e.g. [Cargo.lock]

checks: []                       # shell commands run before a pre-release and in --build-only

release:
  title: "{name} {version}"      # pre-releases append " (pre-release)"
  notes_suffix: ""               # appended to GitHub release notes (not to the commit)
  commit_trailer: ""             # appended to the squash commit, e.g. "[skip ci]"
  extra_assets:                  # zipped from the working tree at pre-release time
    - { zip: examples/, as: MyApp-examples.zip }

build: []                        # build steps, see below

appstore:                        # optional; see "iOS / App Store"
  worktree: appstore
  bundle_id: com.example.myapp
  export_options: bin/AppStoreExportOptions.plist
  project: MyApp.xcodeproj       # default: the xcode-dmg step's
  scheme: MyApp                  # default: the xcode-dmg step's
  build_dir: build/release-ios
  screenshot_max_age_days: 60
  locale: en-US
  notes_forbidden: [Windows, Linux]  # words the App Store notes must not contain
```

Version presets: `xcode` reads the first `MARKETING_VERSION` and `--next`
bumps every occurrence equal to the current version (so test targets at `1.0`
are left alone); `cargo` the first `^version = "…"`; `cmake`
`set(APP_VERSION "…")`.

### Build steps

`build:` is a list. A pre-release starts every CI step first, runs the local
steps while CI builds, then waits for CI and publishes everything in one go.
If any step fails, nothing is published.

| kind | where | what |
|---|---|---|
| `commands` | local | `run:` shell commands, then upload files matching `assets:` globs |
| `xcode-dmg` | local | Xcode archive (arm64) → Developer ID export → DMG (create-dmg) → codesign → notarize + staple (not in `--build-only`) |
| `github-dispatch` | CI | `gh workflow run <workflow> --ref vX.Y.Z-pre`, wait, download the run's artifacts, copy each to its asset name |
| `github-tag-push` | CI | the workflow triggers on the `-pre` tag push and publishes the release itself; `release` only waits for it |

```yaml
build:
  - kind: xcode-dmg
    project: MyApp.xcodeproj
    scheme: MyApp
    app_name: MyApp              # default: scheme
    export_options: bin/ExportOptions.plist   # method developer-id
    build_dir: build/release
    notary_profile: notarytool   # env NOTARYTOOL_PROFILE overrides
  - kind: github-dispatch
    workflow: build.yml
    artifacts:
      - { artifact: myapp-linux-x86_64, file: "MyApp-*.AppImage", as: MyApp-linux-x86_64.AppImage, executable: true }
  - kind: github-tag-push
    workflow: release.yml
  - kind: commands
    run: ["./gradlew clean buildPlugin"]
    assets: ["build/distributions/*.zip"]
```

A `github-dispatch` workflow must already exist on the main branch: GitHub
only dispatches workflows whose file is on the default branch. `release`
checks this before pushing the `-pre` tag. A new workflow therefore goes to
main once by hand (then merge main back into the dev branch).

`xcode-dmg` signs with `$SIGN_IDENTITY` or the first "Developer ID
Application" identity in the keychain. Notarization credentials are stored
once with:

```sh
xcrun notarytool store-credentials notarytool \
  --apple-id "you@example.com" --team-id <TEAMID> --password "<app-specific-password>"
```

If create-dmg fails with error -1743, grant your terminal app *Automation →
Finder* in System Settings → Privacy & Security; `release` waits and retries.

## iOS / App Store

With an `appstore:` section, every macOS pre-release also converts the
CHANGELOG section to App Store plain text, shows it for confirmation, and
commits it as `whatsnew.txt` in the `appstore` worktree, a checkout of an
orphan branch holding the store metadata:

```
appstore/
  metadata/description.txt  keywords.txt  support_url.txt  privacy_url.txt  marketing_url.txt
  screenshots/iphone-NN-*.png (1320×2868)  ipad-NN-*.png (2752×2064)
  whatsnew.txt
```

One-time setup per clone: `git worktree add appstore appstore`.

A CHANGELOG line can be limited to one set of notes with an HTML comment,
which doesn't show where GitHub renders the markdown:

```markdown
- **Windows** New desktop app for Windows. <!-- appstore: skip -->
- **iPad** Wording only for the App Store. <!-- appstore: only -->
```

`skip` lines go only into the GitHub release notes and the squash-merge
message, `only` lines only into `whatsnew.txt`; unmarked lines go into both.
A `### ` heading left without lines is dropped, and an unknown marker is an
error. If the App Store text still contains a word from
`appstore.notes_forbidden`, `release` stops and lists the lines (`--dry-run`
warns), and `release ios --release` checks `whatsnew.txt` again before
uploading it.

- `release ios` archives for iOS, exports the IPA, validates it
  (`SKIP_VALIDATE=1` skips) and uploads it to TestFlight. Requires a macOS
  GitHub release (`vX.Y.Z-pre` or `vX.Y.Z`) for the same version, so both
  platforms ship from the same code.
- `release ios --release` checks the metadata and screenshots, verifies that
  the processed build in App Store Connect has the local
  `CURRENT_PROJECT_VERSION`, creates or finds the App Store version, uploads
  description, keywords, URLs, What's New and screenshots, and attaches the
  build. Submitting for review stays manual.
- `release ios --build-only` only builds the IPA.

App Store Connect API key (once): App Store Connect → Users and Access →
Integrations → App Store Connect API → new key with the "App Manager" role;
put `AuthKey_<KEYID>.p8` into `~/.appstoreconnect/private_keys/` and export
`APP_STORE_API_KEY_ID` and `APP_STORE_API_ISSUER_ID`.

## Signing certificates for CI: `renew-certs`

A second tool, `renew-certs`, rotates the Developer ID certificates that CI
workflows sign and notarize with. Certificates from Apple's G2 Developer ID
Sub-CA are valid only about 13 months, and the original Sub-CA (with every
certificate it issued) expires on 2027-02-01, so this is a yearly chore.

Creating the certificates stays manual: Apple only issues Developer ID
certificates to the Account Holder in the developer portal. `renew-certs
guide` lists the steps. Use the portal with the **G2 Sub-CA**, not Xcode's
Manage Certificates, which issued certificates from the original Sub-CA as
late as October 2026. Everything after that is automated.

It runs in a project's repository root, like `release` (`-C DIR` works too),
and takes its settings from a `certs:` section in that project's
`release.yaml`:

```yaml
certs:
  team_id: ABCDE12345        # the certificate subject's OU must match
  # min_days: 30             # refuse / flag certificates expiring sooner
  # secrets:                 # these are the defaults
  #   application: APPLE_CERT_APPLICATION_P12_BASE64
  #   installer:   APPLE_CERT_INSTALLER_P12_BASE64
  #   password:    APPLE_CERT_P12_PASSWORD
```

- `renew-certs guide` explains the portal steps and offers to open Keychain
  Access and the portal.
- `renew-certs status` lists the Developer ID identities in the keychain
  (SHA-1, G2 or old CA, expiry, whether 1Password has a backup) and what the
  repo's CI has. It exits with 1 when a CI certificate expires within
  `min_days` or isn't recorded yet.
- `renew-certs apply` picks the latest G2 identity of each type from the
  keychain and exports both under a new random password (macOS asks for
  the keychain password twice per certificate, so four times). It then checks the type, team, G2 issuer,
  remaining validity and that each doesn't expire earlier than what CI has
  (`--force` overrides). After a confirmation it sets the two `.p12` secrets
  and the password secret, records `…_SHA1`/`…_EXPIRES` repo variables for
  `status`, and offers to delete the superseded identities from the
  keychain. Run it once in every project with a `certs:` section; the
  certificates stay in the keychain, so the same pair serves every project.
- `renew-certs backup` stores each current identity in 1Password: one
  *Password* item per certificate in the **Private** vault, tagged
  `renew-certs` and titled like `developer-id-application-2027-11-20-47c2819d`
  (type, expiry, SHA-1). 1Password generates the item's password; the `.p12`
  encrypted with it is attached; the export asks for the keychain password
  twice per certificate, as in `apply`. Items that already have their `.p12`
  are skipped, so it can simply be re-run. `apply` reminds you to run it.

The password for CI exists only in memory and in the repo secret: nothing
needs it later, because a lost or broken secret is replaced by running
`apply` again, which exports afresh. Passwords reach openssl, `gh` and `op`
through the environment, stdin or `op`'s own generator, never a command
line. What can't be recreated is the private key in the keychain; that's
what `backup` is for. To restore on another Mac, download the `.p12` from
the 1Password item, double-click it, enter the item's password, then run
`apply`. `--keychain FILE` works on another keychain, and `--dry-run` stops
after the checks.

## Examples

`examples/` has the configurations of the projects using this tool, from a
pure CI build with platform artifacts (Calc-U-1600) to a local macOS +
App Store setup (Calc-U-59).

## Development

```sh
cargo test     # unit tests + tests/lifecycle.rs (temp git repos, fake gh on PATH)
               # + tests/renew_certs.rs (fake gh, throwaway keychain, test certs)
cargo clippy --all-targets
```

## License

Copyright (C) 2026 Martin Erzberger. Licensed under the
[GNU General Public License, version 3](LICENSE) (GPLv3).
