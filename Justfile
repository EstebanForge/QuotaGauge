default:
    @just --list

# Install npm dependencies
install:
    npm ci

# Run Tauri dev server with hot reload
dev:
    npm run tauri dev

# Type check Svelte and TypeScript
check:
    npm run check

# Lint JS/TS/Svelte with ESLint and Prettier
lint:
    npm run lint

# Run frontend unit tests (Vitest)
test:
    npm run test

# Run Rust tests in src-tauri
test-rust:
    cargo test --manifest-path src-tauri/Cargo.toml --all-targets

# Full quality gate: versions + contracts + frontend + rust
verify:
    npm run verify

# Build frontend to dist/
build:
    npm run build

# Build Linux desktop packages (deb, appimage, rpm)
build-linux:
    npm run build:linux

# Local CI verification gate
ci: verify build
