# Changelog

## [0.1.0] - work in progress

- **First version**: one `release` command driven by `release.yaml`, replacing the per-project `bin/release` scripts of Calc-U-59 (macOS + iOS), Calc-U-1600, SharpDataExchange and VolumePruner.
- **Release cycle**: `release` (pre-release under `vX.Y.Z-pre`), `--rc` (dates the CHANGELOG entry), `--release` (squash-merge to main, retarget the pre-release onto `vX.Y.Z` in place), `--next` (cut the next dev branch, bump the version), `--build-only`, `--dry-run`.
- **Build steps**: `commands`, `xcode-dmg` (local Developer ID DMG with notarization), `github-dispatch` and `github-tag-push`, combinable; all assets go into one pre-release.
- **App Store**: `whatsnew.txt` generation during pre-releases, `release ios` for TestFlight and App Store Connect setup (metadata, screenshots, build).
- **`renew-certs`**: a second tool that rotates the Developer ID certificates CI signs with. It exports the newest G2 identities from the keychain under a fresh random password, checks them, sets the `.p12` and password secrets of the project it runs in (configured by a `certs:` section in `release.yaml`), and records SHA-1 and expiry as repo variables; `status` reports what CI has and when it expires, and `backup` stores the identities in 1Password (Private vault).
- **App Store notes**: CHANGELOG lines marked `<!-- appstore: skip -->` stay out of `whatsnew.txt`, lines marked `<!-- appstore: only -->` out of the GitHub notes; `appstore.notes_forbidden` stops a release whose App Store notes still name a listed word (e.g. other platforms).
