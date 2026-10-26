# Development

## Requirements

- Node.js 22 or later
- Stable Rust toolchain
- [Tauri 2 platform prerequisites](https://v2.tauri.app/start/prerequisites/)

## Run the app

```sh
npm install
npm run tauri dev
```

## Quality checks

```sh
npm run verify
```

This runs the version checks, the frontend contract verification, the frontend lint and tests, and the full Rust suite.

## Build an installer

```sh
npm run build:installer             # Windows
npm run build:linux                 # Linux
npm run tauri -- build --bundles dmg   # macOS
```

## Releasing

Maintainers can review updater and optional native-signing requirements in [releasing.md](releasing.md).
