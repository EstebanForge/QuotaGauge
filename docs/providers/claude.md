# Claude Code

QuotaGauge tracks Claude subscription limits.

## What it tracks

| Metric            | Meaning                                                      |
| ----------------- | ------------------------------------------------------------ |
| Session           | Usage remaining in the current session window                |
| Weekly            | Usage remaining in the weekly window                         |
| Sonnet / Fable    | Model-specific limits when they are reported for the account |
| Extra Usage       | Extra-usage allowance reported by Claude                     |
| Rate Limit Resets | Available reset credits                                      |

## Sign-in

Sign in with Claude Code by running `claude`. QuotaGauge reuses the credentials maintained by the
CLI, including `CLAUDE_CONFIG_DIR` when it is set. Refreshed CLI credentials are saved back to the
same source when possible.

## Multiple accounts

QuotaGauge discovers separate Claude Code logins that use custom `CLAUDE_CONFIG_DIR` homes and shows
each account as its own card with independent limits and plan. Logins belonging to the same Claude
account are combined automatically.

Account cards can be renamed from Customize or from the dashboard. If a login is removed, its card
is hidden and returns with its previous customization when the login is detected again.

Live subscription limits currently require a Claude Code login. On macOS, QuotaGauge can recognize
that Claude Desktop is installed, but it does not reuse Desktop's encrypted session. Run `claude`
and sign in once if Desktop is your only Claude login.

## Troubleshooting

- **Not logged in** — run `claude`, complete sign-in, then refresh QuotaGauge.
- **Claude Desktop login found** — sign in once through the Claude Code CLI.
- **Session or token expired** — sign in again with `claude`.
