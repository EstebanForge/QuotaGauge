# Kimi

QuotaGauge tracks the two windows Kimi's own console shows: the rolling 5-hour Session and the Monthly pool. Both are starred for the menu bar by default on fresh setups. If you used an older version, your saved layout is kept and Monthly may need a star in **Customize**.

## What it tracks

| Metric  | Meaning                                      |
| ------- | -------------------------------------------- |
| Session | Usage remaining in the rolling 5-hour window |
| Monthly | Usage remaining in the monthly pool          |

Kimi removed the old 7-day weekly window from their plans and console; QuotaGauge no longer shows it. Upgrades drop the stale Weekly row from the panel automatically.

## Setup

Create a Kimi Code API key in the [Kimi Code Console](https://www.kimi.ai/code/console), then add
it in **Customize** in QuotaGauge. Saved keys are stored in the operating system's credential store.
QuotaGauge also checks `KIMI_API_KEY`, `KIMI_CODE_API_KEY`, and `~/.config/quotagauge/kimi.json`; a
key saved in the app takes priority.

Without an API key, QuotaGauge reuses a local OAuth token, read-only (no refresh), in this order:

1. `~/.kimi-code/credentials/kimi-code.json` — Kimi Code CLI credential (installed from the
   [Kimi download page](https://www.kimi.ai/products/download)); used only while unexpired
2. `~/.pi/agent/auth.json` — the `kimi-coding` entry created by the pi coding agent

Tokens are never refreshed by QuotaGauge; the owning client renews them. If the token expires,
run the Kimi CLI or pi to refresh it. A China-region CLI token fails on the international
endpoint with an authentication error.

This provider uses the Kimi Code endpoint, `https://api.kimi.ai/coding/v1`. Kimi Code keys are not
interchangeable with Kimi Open Platform keys.

## Troubleshooting

- **Add an API key** — add a Kimi Code key in Customize or provide it through a supported external source.
- **API key invalid** — create or verify the key in the [Kimi Code Console](https://www.kimi.ai/code/console).
- **Usage unavailable** — check the connection and refresh again.
