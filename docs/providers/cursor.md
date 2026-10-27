# Cursor

QuotaGauge tracks plan usage from the Cursor account already signed in on your computer.

## What it tracks

| Metric      | Meaning                                              |
| ----------- | ---------------------------------------------------- |
| Total Usage | Overall plan usage for the current billing period    |
| Auto Usage  | Usage assigned to Cursor's Auto model selection      |
| API Usage   | API usage reported by Cursor                         |
| Extra Usage | On-demand usage when it is available for the account |
| Requests    | Included request usage for supported plans           |
| Credits     | Remaining or available credits reported by Cursor    |

## Sign-in

Sign in through the Cursor app or run `agent login`. QuotaGauge looks for Cursor's local application
state and platform credential storage, so no separate QuotaGauge login is required.

## Troubleshooting

- **Not logged in** — open Cursor and sign in, or run `agent login`.
- **Session expired** — sign in again through Cursor or the agent CLI.
- **Some metrics show No data** — Cursor returns different fields for different plans.
