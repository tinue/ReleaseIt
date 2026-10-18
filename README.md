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
./install.sh        # cargo build --release, copy to ~/Applications/release
```

`~/Applications` must be on `PATH`. Prerequisites depend on the project:
`git` and [`gh`](https://cli.github.com) (`gh auth login`) always; Xcode,
`create-dmg` (`brew install create-dmg`), a Developer ID Application identity
and notarytool credentials for local macOS builds; `zip` for `extra_assets`.

## The release cycle

```
release                 pre-release: build, publish/update GitHub pre-release vX.Y.Z-pre
release --rc            release candidate: date the CHANGELOG entry, then as above
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

## Examples

`examples/` has the configurations of the projects using this tool, from a
pure CI build with platform artifacts (Calc-U-1600) to a local macOS +
App Store setup (Calc-U-59).

## Development

```sh
cargo test     # unit tests + tests/lifecycle.rs (temp git repos, fake gh on PATH)
cargo clippy --all-targets
```

## License

Copyright (C) 2026 Martin Erzberger. Licensed under the
[GNU General Public License, version 3](LICENSE) (GPLv3).
