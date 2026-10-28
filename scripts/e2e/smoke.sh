#!/usr/bin/env bash
# End-to-end smoke test: launches the real app binary under Xvfb with an
# isolated HOME, drives the UI with xdotool, and asserts the window, the
# dashboard/settings screens and a clean shutdown. Artifacts (screenshots,
# logs) are copied to the output directory for before/after comparison.
#
# Usage: scripts/e2e/smoke.sh [binary] [artifact-dir]
# The binary must be built with embedded frontend assets (feature
# custom-protocol), otherwise debug builds load the vite devUrl and the
# webview shows a connection error:
#   (cd src-tauri && cargo build --features tauri/custom-protocol)
set -euo pipefail

BIN=${1:-src-tauri/target/debug/quotagauge}
OUT=${2:-}
if [[ ! -e $BIN ]]; then
  echo "FAIL: binary not found: $BIN" >&2
  exit 2
fi
BIN=$(readlink -f "$BIN")

WORK=$(mktemp -d)
trap '[[ -n ${XVFB_PID:-} ]] && kill "$XVFB_PID" 2>/dev/null; rm -rf "$WORK"' EXIT
if [[ -z $OUT ]]; then
  OUT="${TMPDIR:-/tmp}/quotagauge-e2e/$(date +%Y%m%d-%H%M%S)"
fi
mkdir -p "$OUT"

FAILURES=0
fail() { echo "FAIL: $*"; FAILURES=$((FAILURES + 1)); }
pass() { echo "PASS: $*"; }

export HOME="$WORK/home"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_CONFIG_HOME="$HOME/.config"
export GDK_BACKEND=x11
export WEBKIT_DISABLE_DMABUF_RENDERER=1
export DISPLAY=:97
mkdir -p "$HOME" "$XDG_DATA_HOME" "$XDG_CONFIG_HOME"

Xvfb "$DISPLAY" -screen 0 1280x800x24 -nolisten tcp &
XVFB_PID=$!
for _ in $(seq 1 20); do
  [[ -S /tmp/.X11-unix/X${DISPLAY#:} ]] && break
  sleep 0.25
done

INNER="$WORK/inner.sh"
cat >"$INNER" <<INNER
set -uo pipefail
cd "\$HOME"
"$BIN" 2>"$WORK/stderr.log" &
APP_PID=\$!

shot() { ffmpeg -hide_banner -loglevel error -f x11grab -video_size 1280x800 -i "$DISPLAY" -frames:v 1 -y "$OUT/\$1.png" </dev/null; }

sleep 8
WIN=""
for _ in \$(seq 1 5); do
  WIN=\$(xdotool search --name 'QuotaGauge' 2>/dev/null | head -1)
  [[ -n \$WIN ]] && break
  sleep 1
done
if [[ -n \$WIN ]]; then
  xdotool windowactivate "\$WIN" 2>/dev/null || true
  sleep 1
  shot 1-dashboard
  xdotool key ctrl+comma
  sleep 1
  shot 2-settings
  xdotool key ctrl+comma
  sleep 1
  shot 3-dashboard-again
  # xdotool windowclose destroys the X window without a WM relaying
  # WM_DELETE, so quit through the app's own Ctrl+Q binding instead.
  xdotool key ctrl+q
else
  shot 0-no-window
fi

for _ in \$(seq 1 10); do
  kill -0 "\$APP_PID" 2>/dev/null || break
  sleep 1
done
EXIT_CODE=129
if kill -0 "\$APP_PID" 2>/dev/null; then
  echo "still-running after windowclose; sending SIGTERM"
  kill -TERM "\$APP_PID"
  wait "\$APP_PID" 2>/dev/null || EXIT_CODE=\$?
else
  wait "\$APP_PID" 2>/dev/null
  EXIT_CODE=\$?
fi
echo "\$EXIT_CODE" >"$WORK/exit-code"
INNER

bash "$INNER"
EXIT_CODE=$(cat "$WORK/exit-code")
cp "$WORK/stderr.log" "$OUT/stderr.log"
echo "exit-code: $EXIT_CODE"

if [[ -f $OUT/1-dashboard.png ]]; then
  pass "main window mapped (screenshot captured)"
else
  fail "no QuotaGauge window appeared"
fi
if [[ -f $OUT/2-settings.png && -f $OUT/1-dashboard.png ]]; then
  if ! cmp -s "$OUT/1-dashboard.png" "$OUT/2-settings.png"; then
    pass "Ctrl+, switched to the settings screen (screenshots differ)"
  else
    fail "dashboard and settings screenshots are identical; shortcut did nothing"
  fi
else
  fail "settings screenshot missing"
fi

APP_LOG=$(find "$HOME" -type f -name '*.log' 2>/dev/null | head -1)
if grep -qi 'panic' "$OUT/stderr.log" ${APP_LOG:+"$APP_LOG"}; then
  fail "panic found in app logs"
else
  pass "no panic in app logs"
fi
if [[ -n $APP_LOG ]]; then
  cp "$APP_LOG" "$OUT/app-log.txt"
  pass "app log written: ${APP_LOG#"$HOME"/}"
else
  fail "no QuotaGauge-*.log found under the isolated HOME"
fi

if [[ $EXIT_CODE == 0 ]]; then
  pass "clean exit on window close"
else
  fail "exit code $EXIT_CODE (expected 0 from window close)"
fi

echo "artifacts: $OUT"
if [[ $FAILURES -gt 0 ]]; then
  echo "SMOKE RESULT: FAIL ($FAILURES)"
  exit 1
fi
echo "SMOKE RESULT: PASS"
