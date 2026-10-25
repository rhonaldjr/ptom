#!/usr/bin/env bash
# Hand check (Q4, M10) on a computer with a GPU; CI runners have none.
# farsight-agent shares a red X screen with --h264, encoding on the GPU
# (VA-API on AMD/Intel, NVENC on NVIDIA); a Farsight viewer session decodes
# it with Cisco's OpenH264 and finds the red desktop.
# Needs: Xvfb and xsetroot (or E2E_DISPLAY: a 640x480 X display run with -noreset),
# network access for OpenH264, and an agent built with
#   cargo build --release -p farsight-agent -p farsight-passwd --features farsight-agent/h264,farsight-agent/vaapi,farsight-agent/nvenc
set -euo pipefail
cd "$(dirname "$0")/../.."
BIN=${BIN:-target/release}
PORT=${PORT:-15993}
USER_NAME=$(id -un)
PASSWORD=farsight-e2e-pw
INTEROP_TEST=${INTEROP_TEST:-cargo test -q -p farsight-net --test interop_tigervnc --}

XDG_CONFIG_HOME=$(mktemp -d)
XDG_DATA_HOME=$(mktemp -d)
export XDG_CONFIG_HOME XDG_DATA_HOME
LOG=$(mktemp)
AGENT= XVFB=
cleanup() {
  [ -n "$AGENT" ] && kill "$AGENT" 2>/dev/null || true
  [ -n "$XVFB" ] && kill "$XVFB" 2>/dev/null || true
}
trap cleanup EXIT

if [ -z "${E2E_DISPLAY:-}" ]; then
  Xvfb :93 -screen 0 640x480x24 -nolisten tcp -noreset &
  XVFB=$!
  E2E_DISPLAY=:93
  sleep 1
fi
DISPLAY=$E2E_DISPLAY xsetroot -solid red

printf '%s\n' "$PASSWORD" | "$BIN/farsight-passwd" --stdin user add "$USER_NAME"
"$BIN/farsight-agent" --install-openh264
"$BIN/farsight-agent" --check-h264

DISPLAY=$E2E_DISPLAY WAYLAND_DISPLAY= RUST_LOG=info USER=$USER_NAME \
  "$BIN/farsight-agent" --backend x11 ${E2E_AGENT_H264:---h264} --port "$PORT" --no-advertise >"$LOG" 2>&1 &
AGENT=$!
for _ in $(seq 1 50); do (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break; sleep 0.2; done

FARSIGHT_INTEROP_ADDR="127.0.0.1::$PORT" \
FARSIGHT_INTEROP_PASSWORD=$PASSWORD \
FARSIGHT_INTEROP_USERNAME=$USER_NAME \
FARSIGHT_INTEROP_OPENH264_DIR="$XDG_DATA_HOME/farsight" \
  $INTEROP_TEST --nocapture || { echo "agent log:"; tail -40 "$LOG"; exit 1; }
if ! grep -q "hardware H.264 encoding on" "$LOG"; then
  echo "FAIL: the agent didn't encode on the GPU:"; cat "$LOG"; exit 1
fi
grep -o "hardware H.264 encoding on.*" "$LOG" | head -1
echo "GPU H.264 end-to-end: OK"
