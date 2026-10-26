# Changelog

## 0.5.4 - 2026-10-26

### Changed

- Saved API keys now live in a single system credential item, so a new macOS build asks for keychain access once instead of once per saved key. Existing keys move over automatically on first use.

## 0.5.3 - 2026-10-26

The first QuotaGauge release. Everything below happened before at OpenQuota 0.5.0 version and below.

### Changed

- The app is now QuotaGauge, with a new bundle identifier and a new gauge icon on Windows, macOS and Linux.
- First launch imports the settings database and saved API keys from an existing OpenQuota installation. Your OpenQuota install stays untouched.
- The menu bar icon defaults to Bars instead of Text.
- The tray gauge uses the same gauge artwork as the app icon: the arc fills as you use your limits, and the needle stays visible.

### Added

- Antigravity: Argon Weekly meter.
- Cursor: Grok Bot (Cursor Sand) weekly meter.
- Claude: Rate Limit Resets row showing when your limits reset.

### Fixed

- macOS: clicking the tray icon now opens the panel on macOS 27. It used to open the Settings menu instead.
- Antigravity: Gemini Session and Weekly plus Claude meters track live usage again. Quotas now merge both Cloud Code hosts, and when the local language server hides Argon it comes from the agy CLI report instead.
- Antigravity: the plan name resolves from stable tier ids, so plan labels survive display name changes.
- Z.ai: Session and Weekly meters are back after Z.ai renamed the quota entry type on their side.
- OpenRouter: Key Limit now measures the current window instead of lifetime usage, so capped keys no longer overstate spend.
- Cursor: usage events come from the current dashboard endpoint instead of the retired CSV export.
- macOS: expanding a provider card no longer leaves a stray blue focus ring near the header.
