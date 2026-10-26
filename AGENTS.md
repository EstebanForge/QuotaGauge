# AGENTS.md

## Project overview

QuotaGauge is a Tauri 2 desktop app (Rust backend, Svelte 5 frontend) that shows AI coding
subscription usage and quota limits in a menu-bar panel. It is a maintained continuation of
OpenUsage/OpenQuota and supports 12 providers (claude, codex, copilot, cursor, antigravity, devin,
grok, kimi, minimax, opencode, openrouter, zai). Targets Windows, macOS, Linux.

## Setup

Prereqs: Node 22+ (npm included), stable Rust, Tauri 2 platform
prereqs (https://v2.tauri.app/start/prerequisites/).

```sh
npm install
npm run tauri dev
```

## Commands

```sh
npm run verify            # full gate: versions + contracts + frontend + rust
npm run lint              # eslint + prettier --check
npm run check             # svelte-check
npm run test              # vitest run
npm run test:watch
npm run build             # vite build -> dist/
npm run build:linux       # tauri build --bundles deb,appimage
npm run build:installer   # tauri build --bundles nsis (Windows)
npm run tauri -- build --bundles dmg   # macOS
```

Rust gates (what `verify:rust` runs, useful singly):

```sh
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all-targets        # filter: cargo test <name>
```

Single frontend test file: `npm run test -- src/lib/metrics.test.ts`.

Headless e2e smoke (Linux): `scripts/e2e/smoke.sh <binary> <artifact-dir>` — always pass the
artifact dir; the binary must be built with `--features tauri/custom-protocol` (see gotchas).

## Architecture

- `src/` — Svelte 5 frontend. `App.svelte` owns the `AppScreen` union ('dashboard' | 'customize' |
  'settings' | `provider:<id>`) and panel chrome; `components/` the screens' building blocks;
  `lib/` pure TS modules (backend.ts invoke wrappers, types.ts backend-type mirror, pacing,
  metricFormat, pageTransition, windowController) each with a colocated `*.test.ts`.
- `src-tauri/src/` — Rust backend. `lib.rs` wires setup: `providers::build_registry()` (ALL provider
  construction lives there — contract-pinned), refresh loop, tray, single instance.
  - `providers/<name>/` — one module per provider: `client.rs` (HTTP, via shared
    `providers/http.rs`), `mapper.rs` (payload -> `QuotaWindow`/`Metric`), `mod.rs` (provider impl +
    `definition()` MetricDefinition table + tests). Auth for subscription providers reads the
    vendor CLI's local state; API-key providers use `providers/api_key.rs` + OS credential store.
  - `service.rs` — UsageService: refresh orchestration, state machine, event publication.
  - `commands/` — the Tauri IPC surface; returns `Result<T, String>` at this boundary only.
  - `storage.rs` (SQLite at app_data_dir/quotagauge.db), `settings.rs` (settings domain),
    `models.rs` (shared types mirrored in `src/lib/types.ts`).
  - `window.rs`, `tray_presentation.rs`, `menu_bar.rs`, `tray_icon.rs` (non-macOS renderer),
    `desktop_integration.rs` (tray vs floating-window fallback per platform).
  - `legacy.rs` + `providers/api_key.rs` — one-time import of OpenQuota data (`io.github.deviffyy.openquota`).
- `scripts/verify/` — source-inspection contract verifiers (see gotchas). `scripts/e2e/`,
  `scripts/macos-cross/` — smoke harness, .app bundler.
- `docs/` — installation.md, development.md, releasing.md, providers/*.md (per-provider user docs).
- `src-tauri/gen/` — generated. Never hand-edit.

Data flow: providers -> UsageService refresh (events `usage-state`, `open-screen`,
`main-window-hidden`) -> frontend listeners via lib/backend.ts; commands for user actions.

## Conventions & gotchas

- Versions must stay identical in `package.json`, `src-tauri/tauri.conf.json`,
  `src-tauri/Cargo.toml` — bump all three together; `verify:versions` enforces it.
- `scripts/verify/*.js` are NOT in `npm run verify` except `verify:contracts`' five. Run them
  after touching: Tauri commands (`verify-command-contract`), `models.rs` (`verify-model-contract`),
  provider wiring (`verify-provider-registry-contract` regex-asserts the exact
  `Arc::new(XProvider::new)` shape inside `build_registry` in `providers/mod.rs`), single-instance
  setup, and `docs/installation.md` (`verify-platform-smoke` asserts five exact phrases verbatim).
- Any binary meant to run outside `tauri dev` needs `--features tauri/custom-protocol`, or the
  webview serves the vite devUrl and ships a blank window (Linux smoke AND macOS builds).
- macOS hand-delivery: after copying `QuotaGauge.app` into `/Applications`, re-sign it with
  `codesign --force --sign "Apple Development" /Applications/QuotaGauge.app`. The keychain ACLs
  for the single combined API-key item key off the identity's team-anchored designated
  requirement, so this one step is what keeps a new build from re-prompting for keychain access
  (verified 2026-10-26: 0 prompts across builds vs 1 per build ad-hoc). The Apple Development
  cert renews yearly without changing the team ID, so the ACLs survive renewal.
- macOS releases are built AND signed locally on ATTD-Zenless, not on GitHub runners
  (macos-app.yml is disabled): fresh `npm run build`, then in `src-tauri` the cross recipe
  (SDKROOT=~/.local/opt/macos-sdk/MacOSX.sdk, brew clang CC, llvm-ar AR,
  CFLAGS '-isysroot $SDK -mcpu=apple-a14', RUSTFLAGS linker=/tmp/apple-link.sh — recreate it:
  brew clang -target arm64-apple-macos11 -isysroot $SDK -fuse-ld=/usr/bin/ld64.lld
  -Wl,-platform_version,macos,11.0,27.0 — and run cargo via PATH="$HOME/.cargo/bin:$PATH"
  because brew's rust shadows rustup's and lacks the darwin std),
  `scripts/macos-cross/bundle-app.sh`, Apple-sign the .app with
  `rcodesign sign --timestamp-url none --p12-file ~/Dev/Signing-Keys-Certs/apple-dev.p12
--p12-password-file ~/Dev/Signing-Keys-Certs/apple-dev.p12.pass` (Apple's timestamp server is
  unreachable from this box; Apple Development signatures need none), re-tar with
  `QuotaGauge.app/` as the only root, updater-sign with `tauri signer sign`, then upload the
  tarball + .sig + a fresh latest.json (three platform keys) to the release. Never rebuild
  dist after the version bump without rebuilding the binary: vite used to bake package.json's
  version into the UI; the app version now comes from the binary at runtime via getVersion().
- `generate_context!` embeds `dist/` at compile time: rebuild the frontend (`npm run build`)
  before any cargo build that should carry current assets, or you ship stale UI silently.
- Rust style: no `unwrap`/`expect` outside `#[cfg(test)]`; typed errors internally, `String` only
  at the command boundary; early returns; tests colocated in `mod tests` plus `src-tauri/tests/`.
  Provider tests mock HTTP with `test_http::serve_once` (fixed connection budget per mock).
- Frontend: Svelte 5 runes (`$state`), discriminated unions over boolean flags, behavioral tests in
  `App.test.ts`. Reuse the refresh epilogue helpers (`publish_refresh*` in notifications.rs), the
  shared HTTP client (`providers/http.rs`), and `apply_api_key_mutation` instead of re-inlining them.
- `openquota` strings that remain are intentional (legacy migration in legacy.rs, api_key.rs
  LEGACY_SERVICE, README/installation.md migration notes, one log line). Do not "clean them up".
- Repo is currently PRIVATE with CI + Release workflows disabled and Dependabot at 0 (stand-down);
  pushes trigger no runs. Re-enable: `gh workflow enable CI` + `gh workflow enable Release` and
  raise `.github/dependabot.yml`'s `open-pull-requests-limit`. Exception: `macOS app`
  (`.github/workflows/macos-app.yml`) is dispatch-only by design — it signs the arm64 .app zip
  with the Apple Development identity imported from the MACOS_SIGNING_P12 secrets.
- Platform: macOS is an accessory app (LSUIElement, tray-driven); Linux falls back to a floating
  window when no tray host exists (`desktop_integration.rs`, `OPENQUOTA_LINUX_TRAY_HOST` test
  override). Keep platform-specific behavior working on all three OSes.
- Run quality gates as separate invocations (or confirm each tool's output appeared) — chained
  `&&` scripts can short-circuit early and mask failures.

## Commit & PR guidelines

- Conventional Commits: `type(scope): summary` — scopes seen: providers, zai, antigravity, tray,
  ui, ci, deps, release; `docs:`, `chore:`, `test:`, `refactor:` unscoped. Imperative subject
  <=72 chars. No AI attribution or sign-offs.
- PRs (CONTRIBUTING.md): one change per PR, explain problem + solution, add/update tests when
  behavior changes, screenshots for visible UI changes, run `npm run verify` first.
- Never commit credentials, tokens, or local config. Security issues go through SECURITY.md.
