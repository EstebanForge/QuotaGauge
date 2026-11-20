# Antigravity

QuotaGauge tracks the shared Gemini and Claude quota pools reported by Antigravity.

## What it tracks

| Metric        | Meaning                                             |
| ------------- | --------------------------------------------------- |
| Five Hour     | Shared Gemini pool in the rolling 5-hour window     |
| Weekly        | Shared Gemini pool in the weekly window             |
| Argon Weekly  | Separate Gemini 4 Argon weekly quota, when reported |
| Claude        | Shared non-Gemini pool in the rolling 5-hour window |
| Claude Weekly | Shared non-Gemini pool in the weekly window         |

Only the pools available to the signed-in account are shown.

## Sign-in and local data

Sign in through Antigravity or run `agy`. QuotaGauge reuses Antigravity's locally stored credentials
and can also discover a compatible running Antigravity language server. No API key needs to be added
to QuotaGauge.

When the local token files are absent, QuotaGauge reads the `gemini` login keychain item that the
Antigravity stack owns. macOS can raise one access prompt for that foreign item; after it, the
refresh credential is mirrored into QuotaGauge's own keychain item (`xyz.actitud.quotagauge.antigravity`),
so later reads stay silent even when the owning app rotates its credentials. Deleting that mirror
item (or signing out in Antigravity) makes QuotaGauge fall back to the live sources on the next
refresh.

## Troubleshooting

- **Not signed in** — open Antigravity or run `agy`, complete sign-in, then refresh QuotaGauge.
- **Argon Weekly shows No data** — the Argon quota comes from the `agy` CLI. Install it with `brew install --cask antigravity-cli`, run `agy` once to sign in, then refresh. GUI launches do not carry your shell PATH, so QuotaGauge also checks `~/.local/bin`, `/opt/homebrew/bin` and `/usr/local/bin`. Set the `ANTIGRAVITY_CLI_PATH` environment variable to an absolute path to force the location; setting it to an empty value turns the CLI source off.
- **Sign-in expired** — authenticate again in Antigravity or with `agy`.
- **Credentials could not be read** — reopen Antigravity so its local sign-in state can be restored.
- **Usage temporarily unavailable** — wait briefly and refresh again.
