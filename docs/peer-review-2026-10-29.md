# Peer review 2026-10-29 (post-0.6.7)

Full-codebase review: bugs, correctness, concurrency, security, error classification, contract, tests, staleness. Reviewer: external peer (read-only). Spot-verified items are marked. Verdict at the time of writing: needs-work, no blockers, ~20 should-fix items. Items are grouped by cluster, not by the reviewer's original numbering; the original number is kept in brackets for traceability.

Legend: `[x]` fixed, `[ ]` open. Severity: B = blocker, S = should-fix, N = nit.

## Shared HTTP hardening

- [x] [S-1] `providers/claude/client.rs:67,101` parses `response.json()` before the status check. A 401/403/429/5xx with empty or HTML body becomes `InvalidResponse`: 401 never reaches token refresh, 429 loses `retry_after`. Tolerate non-JSON bodies like codex and `http.rs` do. Spot-verified.
- [x] [S-2] `providers/claude/mod.rs:418`, `providers/codex/mapper.rs:222`: server-controlled numbers feed time arithmetic that can panic. `now + Duration::seconds(retry as i64)` panics on huge `Retry-After`. Codex does `now + milliseconds((s*1000.0) as i64)`. `pacing.rs:49-50` has the same class with `period_seconds`. Use checked arithmetic and clamp to ~24h. Spot-verified.
- [x] [S-30] 5xx classification is inconsistent: Kimi, MiniMax, Z.ai, OpenRouter map `RequestFailed(5xx)` to `InvalidResponse`; Grok maps to `Network`; claude maps to `Network`. One rule in shared `http.rs`.
- [ ] [N-32] `codex/mod.rs:316`: unmapped 401 becomes `Network`. `claude/client.rs:141-145`: 400/401 without `invalid_grant` becomes `RequestFailed`. `copilot/client.rs:154`: other 4xx and body-read failure become `InvalidResponse`.
- [ ] [N-33] `claude/mod.rs:398`, `codex/mod.rs:224`: 403 triggers token refresh and rotation on every poll, including Cloudflare 403s.

## Credential storage

- [x] [S-17] `providers/api_key.rs:168-203`: `CombinedSecretBackend::write`/`delete` treat any `load()` error as a corrupt map and start empty. A keychain read failure while saving key B replaces the item with `{B}` and loses every other saved key. Replace the map only on JSON parse failure; propagate backend errors. Spot-verified.
- [x] [S-23] `providers/claude/auth.rs:187,249-273`: rewriting `.credentials.json` through a struct that models only `claudeAiOauth` drops sibling keys (`mcpOAuth`) and replaces symlinks with regular files. Codex keeps the whole `Value` document; copy that pattern. Also: the "read-only reuse" project rule disagrees with the claude/codex/grok refresh code; reconcile guide and behavior.
- [x] [S-24] `providers/claude/mod.rs:292-301`, `codex/mod.rs:164-178`, `grok/mod.rs:136-195`: refresh-token races. If the vendor CLI refreshes between our load and refresh, we get `invalid_grant` and show a false "sign in again". Reload and retry once. A failed save after rotation leaves the on-disk token revoked.

## Startup and persistence

- [x] [S-37] `storage.rs:173-183`, `settings.rs:102`, `lib.rs:364-371`: undecodable settings row or unopenable DB propagates out of setup; app fails to start on every launch (e.g. after a downgrade that added an enum variant). Heal like the snapshot loaders: log, back up, fall back to defaults. Only `LogLevel` has `#[serde(other)]`.
- [x] [S-38] `legacy.rs:51`: legacy import copies only `openquota.db`, not `-wal`/`-shm`, in WAL mode. Partial copy plus `NotNeeded` on next launch opens a broken DB. Copy atomically (temp + rename), checkpoint or back up through SQLite.
- [x] [S-39] `xdg_autostart.rs:11-13`: `Exec=` uses `current_exe()`. Inside an AppImage that is `/tmp/.mount_XXXX`, gone after exit; launch-at-login silently dies after every self-update. Write `$APPIMAGE` when set. `quote_exec` also does not escape `%` as `%%`.
- [ ] [N-40] No `PRAGMA user_version`; `schema_version` forced to 7 on load so a downgrade is silently accepted; `settings.rs:163` returns defaults on poisoned lock (every provider reads as disabled).

## Connectivity gate

- [x] [S-14] `connectivity.rs` + `refresh_loop.rs:19`: the gate is a session latch, boot-only. After sleep+wake the same Wi-Fi-not-ready failure recurs and the provider enters the 60s backoff. Reset the latch when a Network-kind failure is observed.
- [x] [S-15] Single-host probe (`www.gstatic.com`): blocked networks (mainland China, strict DNS filters) burn the 30s budget on every launch. Add a second probe target. (Plain-HTTP choice itself is deliberate; not in scope.)

## Process and timeout handling

- [ ] [S-16] `service.rs:302-328`: a timed-out worker keeps `runner_active = true` until the blocking call returns. A never-returning call (keychain prompt, hung child) locks the provider out for the session, and even a forced refresh returns the timeout error. Add a hard ceiling or visible stuck state.
- [ ] [S-18] `child_process.rs:51-75`: on timeout only the direct child is killed, then `join_reader` waits for EOF. A grandchild holding the pipe defeats the deadline. Kill the process group; bound the reader join.
- [ ] [S-19] `providers/antigravity/mod.rs:332-465`: without Argon, each refresh spawns up to 4 `agy` candidates x 2 subcommands at 15s (up to ~60s hung). Negative-cache the no-Argon result for a few minutes.
- [ ] [N-20] SQLite on the async runtime: `settings.rs:357-417` holds a std write guard across SQLite writes; `Storage` mutex across queries, no `busy_timeout`; `api_key.rs` std mutex across Secret Service calls; each Linux keyring op opens a new connection and a miss costs 3 lookups.
- [ ] [N-21] Frontend async races: `App.svelte:633-640` usage-state overwrites bootstrap result (settings protected by revision, usage not); `windowController.svelte.ts:141-172` lost `cancelPendingResize` during awaits; `App.svelte:116-128` auto-update effect re-arms on every settings identity change; `commands/settings.rs:468-488`, `updates.rs:307-309` drop child handles without `wait()` (zombies).

## Security details

- [ ] [S-25] `providers/antigravity/client.rs:82-137`: local client uses `danger_accept_invalid_certs(true)`, follows redirects, honors proxy env vars. The CSRF header could ride a redirect or go to an HTTP proxy. `redirect(Policy::none())`, `no_proxy()`, add `connect_timeout`.
- [ ] [N-26] `logging.rs:508`: redaction misses `gho_`/`ghp_` (Copilot), `xai-` (Grok), Z.ai keys. `redact_value` exposes 8 of 13 chars on short tokens. Log file and DB created 0644; make them 0600.
- [ ] [N-27] `updates.rs:311` returns raw `io::Error` text to the webview; `updates.rs:94-120` classifies by substring (`"403"`, `"429"`) before the typed minisign check.
- [ ] [N-28] `commands/settings.rs:79-93` vs `settings.rs:735-739`: shortcut registered from untrimmed IPC value but the trimmed one is stored. `SettingsScreen.svelte:119-121` can record Space as `Ctrl+ `.
- [ ] [N-29] `known_provider_ids` and `dismissed_update_version` have no length limits. `home_directory()` copied in five places; four use `unwrap_or_default()` and read/write `./.claude/...` relative to cwd when `HOME` is unset. Share one helper that fails closed.

## Provider mapper and logic fixes

- [ ] [S-3] `antigravity/mapper.rs:60-135`, `antigravity/mod.rs:138,251`: `parse_quota_summaries` returns `Some(vec![])` when groups exist but no known bucket matches. A local server answering `{"groups":[]}` yields `Ok` with zero quotas and the GetUserStatus/cloud fallbacks never run. Empty result should be `None`.
- [ ] [S-4] `providers/kimi/local_credentials.rs:21`: `cli_credential(home).or_else(|| pi_credential(home))` only falls through on `None`. A stale CLI file returns `Some(Expired)` so a fresh pi token is never read. Prefer `Fresh` from either source.
- [x] [S-5] `providers/grok/mod.rs:124-133`: when the proactive refresh fails and the JWT is expired, returns `Expired` mapped to `Authentication`; offline the user sees "Grok login expired" instead of a network error. Return `Expired` only when the refresh error is auth-class.
- [ ] [S-6] `providers/copilot/auth.rs:332-366`: `yaml_value` ignores indentation depth; in multi-account `hosts.yml` with insecure storage the first `users.<name>.oauth_token` can win over the active account.
- [ ] [S-7] `providers/claude/mod.rs:448` vs `468-483`: a failed reset-grants fetch caches `status: None` for an hour; the "Rate Limit Resets" row disappears after one transient error.
- [ ] [S-8] `providers/antigravity/discovery.rs:230-241`: macOS `env_csrf_token` runs `ps -ax -E` and takes the first line containing the CSRF var from ANY process. Scope it like the Linux `/proc/{pid}/environ` path.
- [ ] [S-10] `providers/codex/reset_claim.rs:50-129`, `commands/usage.rs:15-23`: the only flow that spends vendor credit. Cache stores only `credit_id`, not the account index; retry with the same request id tries candidate 0 first; list phase returns `NoCredit` after the first non-401 candidate; claim path never refreshes an expired token; credits matched by expiry within 1s can redeem a different credit than the one clicked.
- [ ] [S-11] `providers/minimax/mapper.rs:94-103`: `weekly_boost_permille: 0` gives 0/0 = NaN, serialized as null. Filter `permille > 0`.
- [ ] [S-31] `providers/minimax/mapper.rs:34-40`, `zai/mod.rs:130-132`: only two vendor error messages special-cased; other error envelopes with HTTP 200 become `InvalidResponse`, so a bad key looks like an outage. Confirm against real responses first.
- [ ] [N-12] Smaller mapper items: antigravity missing `remainingFraction` defaults to 0.0 dragging a pool to 100% used; antigravity 99.5-99.6% displays as 100% without being marked spent; codex `header_number` accepts non-finite; claude `number()` accepts `"NaN"`/`"inf"`; claude `resets_at: 0` becomes 1970; codex keeps elapsed reset-credit entries and duplicates `parse_expiry` rounding; kimi `session_quota(body).ok().flatten()` swallows a corrupt session section; minimax missing weekly field fails the whole refresh; `pacing.ts:131` prints "2d 0h"; `UpdateBanner.svelte:15` hidden when both versions are null.

## Reset claim, service, commands

- [ ] [S-13] `service.rs:186-188,195-198`: two `.expect("active refresh generation must exist")` in production code. Restructure with a `match` on `attempt_generation`.
- [ ] [N-34] Production `unwrap`/`expect`/`panic!` outside tests: `menu_bar.rs:100,135,268,417`, `logging.rs:503-560`, `lib.rs:501`, `settings.rs:896-898`, `api_key.rs:386`. Enable `clippy::unwrap_used` + `clippy::expect_used` crate-wide, allowed under `cfg(test)`.
- [ ] [N-35] `commands/usage.rs:58`: `refresh_usage` returns `Result<_, ()>` and never errs; breaks the String-at-boundary convention.
- [ ] [N-36] `openrouter/mod.rs:184-206`: failed endpoint metrics dropped with no warning; missing `total_credits` shows as "$0 Balance".
- [ ] [N-41] `service.rs:421`, `App.svelte:819`: `last_full_refresh_at` stamped even when every provider failed or was skipped; manual refresh does not reset the loop timer; interval duplicated in TS and Rust.
- [ ] [N-42] `pacing.rs:354-362` + `codex/mapper.rs:222`: `resets_at` computed as `now + relative seconds` per refresh; rounding plus latency can clear `fired`/`previous` and repeat alerts.

## Frontend

- [ ] [S-9] `src/lib/shareCard.ts:237,244-258`: copied share card disagrees with the dashboard in three ways (spent-vs-left wording, "Limit reached" on null `runOutAt` at 99.5-100% projected, amber-vs-neutral at 85%). One shared row builder with `QuotaMetric.svelte`.
- [ ] [N-a11y] `ConfirmationSheet` focus trap: Escape triggers `back()` with the sheet open after backdrop click. Footer options menu has `role="menu"` without arrow keys. Drag grip is a button with no Enter/Space action. Every provider header is an `h1`.
- [ ] [N-updates] Manual "Check for Updates" silently dropped while a check/install runs; `installing` never resets after a successful install without relaunch.
- [ ] [N-misc] Background update check clears `settingsError` via `saveSettings`; `shareCard.ts:332-343,482` no ellipsis on 48-char names, `ellipsize` can split an emoji surrogate pair; `windowController.svelte.ts:198-201` one failed fit disables auto-fit until restart; `ProviderLinksExpander.svelte` is dead code (Dashboard inlines the same markup); `shareController.svelte.ts:10` toast timer not cleared on unmount.

## Tests and engineering hygiene

- [ ] [N-t1] Duplicated HTTP client setup: `claude/client.rs:28-35` bypasses `providers/http.rs::build_client` (no connect timeout, hardcoded spoofed UA). Codex and Copilot build their own too.
- [ ] [N-t2] Weak/flaky tests: `claude/accounts.rs:657` 100ms budget can pass by timeout; `claude/mod.rs:869-881` blocking `accept()` with no timeout can hang; `zai/mod.rs:383` fixed port `127.0.0.1:1`; `antigravity/mod.rs:783` test name asserts the opposite; `grok/client.rs:162-183` test-only retry production lacks; `test_http.rs:35-36` single 4096-byte read can RST split POSTs; kimi mapper test hides the swallowed error in S-4's neighbor item; `codex/auth.rs:410-423` tests only one predicate.
- [x] [N-t3] Docs/config drift: AGENTS.md says `build:linux` bundles `deb,appimage`, actual is `deb,appimage,rpm`; `codex/auth.rs:195-205` save lacks `sync_all` (claude has it); Dependabot at 0 and CI disabled so rot is unmonitored; no `cargo audit`/`npm audit` step.
- [x] [N-t4] Connectivity gate tests: `probe_endpoint_is_the_google_empty_reply_check` asserts constants against themselves; `late_joiner_aborts_once_another_caller_settles` fixed 500ms sleep can flake on loaded CI; `.unwrap_or(false)` on the probe join swallows panics silently.
- [ ] [N-csp] `tauri.conf.json:34` CSP has `style-src 'unsafe-inline'`.
