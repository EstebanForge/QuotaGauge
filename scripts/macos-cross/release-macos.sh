#!/usr/bin/env bash
# Full local macOS release leg: fresh frontend build, cross-compile, bundle,
# Apple-sign, tar, updater-sign. Usage: release-macos.sh [output-dir]
# Optional: RELEASE_UPLOAD=1 also uploads the tarball + .sig to the GitHub
# release named after the version (repo resolved from origin).
# Every step guards a failure this pipeline actually hit once:
#   - brew's rust shadows rustup's rustc (no darwin std) -> PATH fix below
#   - stale dist/ baked an old version into the UI -> npm run build is mandatory
#   - the rustc linker script is ephemeral -> recreated under the output dir
#   - Apple's timestamp server is unreachable here -> rcodesign --timestamp-url none
set -euo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$REPO_ROOT"
PATH="$HOME/.cargo/bin:$PATH"
export PATH

OUT=${1:-${TMPDIR:-/tmp}/quotagauge-macos-release}
SDK=${SDKROOT_OVERRIDE:-$HOME/.local/opt/macos-sdk/MacOSX.sdk}
P12=${P12_FILE:-$HOME/Dev/Sign-Keys-Certs/apple-dev.p12}
P12_PASS=${P12_PASSWORD_FILE:-$HOME/Dev/Sign-Keys-Certs/apple-dev.p12.pass}
UPDATER_KEY=$HOME/.tauri/quotagauge-updater.key

for cmd in cargo npm node clang llvm-ar rcodesign openssl tar gh; do
  command -v "$cmd" >/dev/null || { echo "missing command: $cmd" >&2; exit 1; }
done
command -v ld64.lld >/dev/null || { echo "missing command: ld64.lld" >&2; exit 1; }
for file in "$SDK/System" "$P12" "$P12_PASS" "$UPDATER_KEY" \
  "$REPO_ROOT/src-tauri/icons/icon.icns" "$REPO_ROOT/scripts/macos-cross/bundle-app.sh"; do
  [[ -e $file ]] || { echo "missing: $file" >&2; exit 1; }
done

VERSION=$(grep -oP '(?<="version": ")[^"]+' src-tauri/tauri.conf.json | head -1)
[[ -n $VERSION ]] || { echo "could not read version from src-tauri/tauri.conf.json" >&2; exit 1; }

mkdir -p "$OUT"
LINKER="$OUT/apple-link.sh"
cat > "$LINKER" <<EOF
#!/usr/bin/env bash
exec $(command -v clang) -target arm64-apple-macos11 \\
  -isysroot "$SDK" \\
  -fuse-ld=$(command -v ld64.lld) -Wl,-platform_version,macos,11.0,27.0 "\$@"
EOF
chmod +x "$LINKER"

echo "==> frontend dist (fresh; generate_context! embeds it at compile time)"
npm run build >/dev/null

echo "==> cross-compiling for aarch64-apple-darwin"
(
  cd src-tauri
  SDKROOT="$SDK" \
    CC_aarch64_apple_darwin="$(command -v clang)" \
    AR_aarch64_apple_darwin="$(command -v llvm-ar)" \
    CFLAGS_aarch64_apple_darwin="-isysroot $SDK -mcpu=apple-a14" \
    RUSTFLAGS="-C linker=$LINKER" \
    cargo build --release --target aarch64-apple-darwin --features tauri/custom-protocol
)

echo "==> bundling and signing"
bash scripts/macos-cross/bundle-app.sh \
  src-tauri/target/aarch64-apple-darwin/release/quotagauge "$OUT/app"
APP="$OUT/app/QuotaGauge.app"
rcodesign sign --timestamp-url none \
  --p12-file "$P12" --p12-password-file "$P12_PASS" "$APP" >/dev/null
rcodesign verify "$APP/Contents/MacOS/quotagauge"

TARBALL="$OUT/QuotaGauge-$VERSION-macos-arm64.tar.gz"
tar -czf "$TARBALL" -C "$OUT/app" QuotaGauge.app

echo "==> updater signature"
TAURI_SIGNING_PRIVATE_KEY="$(cat "$UPDATER_KEY")" \
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" \
  node_modules/.bin/tauri signer sign "$TARBALL" >/dev/null

if [[ ${RELEASE_UPLOAD:-0} == 1 ]]; then
  echo "==> uploading to release $VERSION"
  gh release upload "$VERSION" --clobber "$TARBALL" "$TARBALL.sig"
fi

echo "==> done (v$VERSION)"
ls -la "$TARBALL" "$TARBALL.sig"
sha256sum "$TARBALL"
echo "upload: RELEASE_UPLOAD=1 $0, or: gh release upload $VERSION $TARBALL $TARBALL.sig --clobber"
