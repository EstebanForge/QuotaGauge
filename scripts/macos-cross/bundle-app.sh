#!/usr/bin/env bash
# Assembles QuotaGauge.app from a macOS-cross-compiled binary.
# Usage: bundle-app.sh <binary> [output-dir]
# The binary must be built for aarch64/x86_64/universal2-apple-darwin.
set -euo pipefail

BIN=""
OUT=""
if [[ $# -ge 1 ]]; then BIN=$1; fi
if [[ $# -ge 2 ]]; then OUT=$2; fi
if [[ -z $BIN || ! -e $BIN ]]; then
  echo "usage: $0 <binary> [output-dir]" >&2
  exit 1
fi
BIN=$(readlink -f "$BIN")

APP_NAME=QuotaGauge
VERSION=$(grep -oP '(?<="version": ")[^"]+' src-tauri/tauri.conf.json | head -1)
OUT=${OUT:-dist-cross}
APP_DIR="$OUT/$APP_NAME.app"
CONTENTS="$APP_DIR/Contents"
ICON_SRC=$(dirname "$0")/../../src-tauri/icons/icon.icns

if [[ ! -e $ICON_SRC ]]; then
  echo "missing $ICON_SRC" >&2
  exit 1
fi

# Renewal nag: macOS assemblies all pass through here, so this is where an
# aging Apple Development cert gets noticed (renewal itself is a manual,
# Xcode-only step on free Apple IDs). The PEM is the public cert only.
CERT_PEM=${HOME}/.local/opt/apple-dev-cert.pem
if [[ -r $CERT_PEM ]]; then
  end_date=$(openssl x509 -in "$CERT_PEM" -noout -enddate | cut -d= -f2)
  end_epoch=$(date -d "$end_date" +%s)
  days_left=$(((end_epoch - $(date +%s)) / 86400))
  if ((days_left < 60)); then
    echo "WARNING: Apple Development cert expires in ${days_left} days ($end_date)." >&2
    echo "Renew in Xcode > Settings > Accounts > Manage Certificates, then re-export the PEM here." >&2
  fi
fi

rm -rf "$APP_DIR"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>xyz.actitud.quotagauge</string>
    <key>CFBundleExecutable</key>
    <string>quotagauge</string>
    <key>CFBundleIconFile</key>
    <string>icon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

cp "$BIN" "$CONTENTS/MacOS/quotagauge"
chmod +x "$CONTENTS/MacOS/quotagauge"
cp "$ICON_SRC" "$CONTENTS/Resources/icon.icns"

echo "assembled $APP_DIR (v$VERSION)"
file "$CONTENTS/MacOS/quotagauge"
