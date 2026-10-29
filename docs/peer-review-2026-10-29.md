# Peer review 2026-10-29 (post-0.6.7, shipped in 0.6.8)

Full-codebase review: bugs, correctness, concurrency, security, error classification, contract,
tests, staleness. Verdict at the time: needs-work, no blockers. The sweep closed 36 of 44
findings across 10 commits (c986bcb..de6fabd); each cluster was one commit. The fixed items are
summarized below by cluster. The remaining items follow in full, including the reason each was
deferred, partial, or rejected. Severity: B = blocker, S = should-fix, N = nit.

## Fixed

- Shared HTTP (S-1, S-2, S-30): status read before body parsing, server time values clamped and
  checked, one 5xx-to-Network rule across providers.
- Credential storage (S-17, S-23, S-24, S-5, N-t3-sync): credential-store backend errors fail
  closed instead of wiping saved keys, vendor files keep unknown keys and symlinks, concurrent
  vendor token rotation is adopted instead of a false sign-out prompt.
- Startup and persistence (S-37, S-38, S-39): undecodable settings and unopenable databases heal
  with forensics instead of bricking launch, legacy import copies WAL sidecars atomically,
  AppImage autostart uses `$APPIMAGE` and escapes `%`.
- Connectivity gate (S-14, S-15, N-t4): the latch re-arms on Network-kind failures (sleep/wake),
  a second probe target was added, tests stop asserting constants and can no longer flake.
- Process and timeouts (S-16, S-18, S-19, N-20 partial): stuck workers force-release after a hard
  ceiling, child timeouts kill the whole Unix process group, Argon misses are negative-cached,
  SQLite has busy_timeout.
- Security (S-25, N-26, N-27, N-28, N-29): antigravity localhost client no-redirect/no-proxy,
  more token prefixes redacted with a tighter partial window, logs and DB owner-only, typed
  update-error classification, one fail-closed home_directory helper.
- Provider mappers (S-3, S-4, S-6, S-7, S-8, S-11, N-12 Rust side): empty-group fallback gone,
  fresh pi token beats a stale CLI token, indentation-aware hosts.yml, pid-scoped CSRF lookup,
  reset-grants row survives one failure, minimax NaN gone, non-finite server values filtered.
- Service and commands (S-13, N-34, N-36, N-41): no unwrap/expect outside tests is
  compiler-enforced, OpenRouter warns on partial failure and no longer fabricates a zero balance,
  the last-refresh stamp requires a live provider.
- Frontend (S-9, N-21, N-misc, N-a11y partial, N-updates partial): share card agrees with the
  dashboard, bootstrap no longer rewinds newer usage state, window-fit cancel race closed,
  auto-fit survives single failures, zombie OS helpers reaped, dead component deleted, Escape no
  longer closes the screen behind an open sheet, provider headers are h2.
- Tests and hygiene (N-t1, N-t2 partial, N-t3): shared client builder used everywhere it applies,
  the claude test server is deadline-bounded so a failing provider cannot hang the suite, AGENTS.md
  matches the real build bundles.

## Open, partial, or rejected

- [ ] [N-32] `codex/mod.rs:316`: unmapped 401 becomes `Network`. `claude/client.rs:141-145`:
  400/401 without `invalid_grant` becomes `RequestFailed`. `copilot/client.rs:154`: other 4xx and
  body-read failure become `InvalidResponse`.
- [ ] [N-33] `claude/mod.rs:398`, `codex/mod.rs:224`: 403 triggers token refresh and rotation on
  every poll, including Cloudflare 403s. (Claude now rotates only on 401; codex 403 still does.)
- [ ] [N-40] No `PRAGMA user_version`; `schema_version` forced to 7 on load so a downgrade is
  silently accepted; `settings.rs:163` returns defaults on poisoned lock (every provider reads as
  disabled).
- [~] [N-20] SQLite on the async runtime: busy_timeout done; holding std write guards across
  SQLite writes and `Storage` mutex across queries deferred as a low-value refactor.
- [~] [S-10] Codex reset claim: account-index cache and list-phase scan fixed; token refresh on a
  401 during claim deferred (needs the codex refresh machinery inside the claim service);
  expiry matching kept at +/-1s.
- [ ] [S-31] Only two minimax/zai vendor error messages special-cased; other envelopes with HTTP
  200 become `InvalidResponse`. Deferred: needs confirmation against real vendor responses.
- [~] [N-12] Rust-side mapper items fixed (NaN/inf, resets_at 0, elapsed credits, pool minimum,
  kimi session swallow, minimax weekly absence); display-side items landed in the frontend
  cluster. codex/claude `parse_expiry` duplication left as-is (consolidation too invasive).
- [ ] [N-35] (invalid: Tauri requires async commands with reference inputs to return Result; the
  shape is framework-mandated) `commands/usage.rs:58`: `refresh_usage` returns `Result<_, ()>`.
- [ ] [N-42] (wontfix: speculative, no observed misfire; the 1000ms re-arm tolerance has headroom)
  `pacing.rs:354-362`: `resets_at` computed as `now + relative seconds` per refresh; rounding plus
  latency can clear `fired`/`previous` and repeat alerts.
- [~] [N-a11y] Fixed: Escape behind an open sheet, provider headers h2. Left: footer options menu
  has `role="menu"` without arrow keys; drag grip keeps its Alt+Arrow shortcut (Enter/Space has no
  reorder semantic).
- [~] [N-updates] Fixed: manual check says why it was dropped. Rejected: `installing` reset after
  install resolves breaks the test contract (progress legitimately renders after resolve).
- [~] [N-t2] Fixed: claude accept/read timeouts. Left: zai port 1 kept (refused connect is
  deterministic and instant); antigravity test name checked and matches its assertions;
  test_http single-read and grok test-only retry left (harness internals, no observed flakes).
- [ ] [N-csp] (wontfix: Svelte injects component styles, so removing style-src unsafe-inline
  would break styling; risk exceeds benefit for a local webview surface).
- [~] [N-t3] Docs drift and sync_all fixed. Left: Dependabot at 0 with CI disabled, and no
  cargo/npm audit step (workflows intentionally stand down while the repo is private).
