# Releasing QuotaGauge

Releases are built per platform by the paths below and published as one GitHub release. CI, Release,
and macos-app workflows stay disabled while the repo is private; the only active workflow is
dispatch-only `Desktop bundles`, which builds the Linux and Windows legs. The macOS arm64 leg is
built and signed locally. Updater trust is a separate layer from native signing: every updater
artifact is signed with `TAURI_SIGNING_PRIVATE_KEY` (or the local updater key file) and verified
against the public key in `src-tauri/tauri.conf.json` before publication.

## Pre-release gates

Run both gates before tagging. Unit tests cannot see the setup hook, and a definition invariant
violation aborts startup before any window appears (0.6.3 shipped exactly that).

```sh
npm run verify
scripts/e2e/smoke.sh src-tauri/target/debug/quotagauge /tmp/qg-smoke
```

Build the smoke binary with embedded assets first (`cd src-tauri && cargo build --features
tauri/custom-protocol`); without the feature the webview loads the vite devUrl and the smoke
silently tests the wrong thing.

## Version bump

Bump all three version fields together; `scripts/verify/verify-versions.js` enforces equality.
`npm version <v> --no-git-tag-version` also repairs package-lock.json; edit `tauri.conf.json` and
`Cargo.toml` by hand with a single-line change (a full JSON rewrite re-indents the dmg block), then
`cargo update -p quotagauge` syncs Cargo.lock. Commit `chore: bump version to <v>` and push main.

## Platform legs

Dispatch `Desktop bundles` from main (`gh workflow run 397210994 --ref main`) for the Linux
(deb, rpm, AppImage) and Windows (NSIS) bundles; download the six artifacts when it finishes. Run
`./scripts/macos-cross/release-macos.sh` locally for the macOS arm64 tarball and updater
signature. Machine-specific signing detail (Apple Development p12, rcodesign flags, updater key
location) lives in AGENTS.md.

## Assemble, verify, publish

Verify every updater asset cryptographically before upload. Build the scratch verifier in
`/tmp/qg-verify` (crate `minisign-verify` 0.2): the pubkey in tauri.conf.json and the `.sig` files
are base64 wrapping the minisign text, so decode-or-pass-through both, then
`PublicKey::verify(whole_file_bytes, signature, true)`. Validate the verifier once against the
previous release's published AppImage plus `.sig`.

Create the release as a draft named after the bare version. Assemble latest.json from the previous
manifest, keeping the three platform keys and putting each `.sig` file's content into `signature`
verbatim (double base64-encoding it breaks every updater install; this happened in 0.6.2). Upload
latest.json under exactly that filename, then the tarballs, installers, and `.sig` files.

Publish, then re-download the published updater assets from the release URLs and re-verify all
signatures against the tauri.conf.json pubkey. Draft-release assets cannot be downloaded through
the REST asset endpoint before publish, so pre-publish verification uses the local files and
post-publish verification uses the published bytes. Confirm the macOS tarball sha256 matches the
local build. Signature-nonempty plus HTTP 200 is not verification.
