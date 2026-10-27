# Codex

QuotaGauge tracks Codex subscription limits.

## What it tracks

| Metric               | Meaning                                                      |
| -------------------- | ------------------------------------------------------------ |
| Session              | Usage remaining in the current session window                |
| Weekly               | Usage remaining in the weekly window                         |
| Spark / Spark Weekly | Model-specific limits when they are reported for the account |
| Extra Usage          | Additional usage credits reported by Codex                   |
| Rate Limit Resets    | Available reset credits                                      |

## Sign-in

Sign in with the Codex CLI by running `codex` and choosing your ChatGPT account. QuotaGauge reads the
same authentication data and respects `CODEX_HOME` when it is set. A ChatGPT login is required for
subscription limits.

## Troubleshooting

- **Not logged in** — run `codex`, sign in with ChatGPT, then refresh QuotaGauge.
- **Subscription usage unavailable** — replace an API-key-only login with a ChatGPT login.
- **Session expired or revoked** — sign in again with `codex`.
