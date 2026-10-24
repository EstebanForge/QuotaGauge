# Development

## Requirements

- Node.js 22 or later
- pnpm 11.11.0
- Stable Rust toolchain
- [Tauri 2 platform prerequisites](https://v2.tauri.app/start/prerequisites/)

## Run the app

```sh
corepack pnpm install --frozen-lockfile
corepack pnpm tauri dev
```

## Quality checks

```sh
corepack pnpm verify
```

This runs the version checks, the frontend contract verification, the frontend lint and tests, and the full Rust suite.

## Build an installer

```sh
corepack pnpm build:installer             # Windows
corepack pnpm build:linux                 # Linux
corepack pnpm tauri build --bundles dmg   # macOS
```

## Releasing

Maintainers can review updater and optional native-signing requirements in [releasing.md](releasing.md).
