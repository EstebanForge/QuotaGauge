# Installation

## Download

| Platform | Available builds                           | Download                                                                      |
| -------- | ------------------------------------------ | ----------------------------------------------------------------------------- |
| Windows  | x64 and ARM64 installers                   | [Download for Windows](https://github.com/EstebanForge/QuotaGauge/releases/latest) |
| macOS    | Universal DMG for Apple Silicon and Intel  | [Download for macOS](https://github.com/EstebanForge/QuotaGauge/releases/latest)   |
| Linux    | x64 and ARM64 AppImage and Debian packages | [Download for Linux](https://github.com/EstebanForge/QuotaGauge/releases/latest)   |

Open the latest release and pick the file for your platform:

- **Windows:** `_x64-setup.exe` or `_arm64-setup.exe`
- **macOS:** `_universal.dmg` (requires macOS 11 or later)
- **Linux:** `.AppImage` or `.deb`

QuotaGauge checks for updates automatically. Update payloads are cryptographically signed with the project's updater key, independent of operating-system package signing.

## Package signing

Native signing is disabled by default today. Windows installers are not Authenticode-signed, and the macOS app uses an ad-hoc signature without Apple notarization. Windows SmartScreen and macOS Gatekeeper may therefore ask for confirmation.

Download QuotaGauge only from this repository's official release page. On macOS, manual approval may be required under Privacy & Security. Each release states its exact native-signing status in the release notes.

## Coming from OpenQuota

On first launch, QuotaGauge imports your existing OpenQuota settings database and saved API keys automatically. Your OpenQuota installation and its data stay untouched.

## API keys

Most providers use credentials already available on your computer. OpenRouter, Z.ai, Kimi, and MiniMax need an API key, which you add in Customize. QuotaGauge stores it in your operating system's credential store.

Codex subscription limits require a ChatGPT login and are not available in API-key-only sessions.
