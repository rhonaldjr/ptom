#!/usr/bin/env bash
# End-to-end (Q4): farsight-agent shares an Xvfb display; a Farsight viewer
# session connects over an encrypted security type, signs in as the owner
# (no approval prompt), and decodes the red desktop.
# Needs: Xvfb, xdpyinfo, xsetroot; binaries in $BIN (default target/debug).
set -euo pipefail
cd "$(dirname "$0")/../.."
BIN=${BIN:-target/debug}
DISPLAY_NUM=${DISPLAY_NUM:-:97}
PORT=${PORT:-15990}
USER_NAME=farsight-e2e
PASSWORD=farsight-e2e-pw

Xvfb "$DISPLAY_NUM" -screen 0 640x480x24 -nolisten tcp >/dev/null 2>&1 &
XVFB=$!
AGENT=
cleanup() { kill $XVFB ${AGENT:+$AGENT} 2>/dev/null || true; }
trap cleanup EXIT
for _ in $(seq 1 50); do DISPLAY=$DISPLAY_NUM xdpyinfo >/dev/null 2>&1 && break; sleep 0.2; done

XDG_CONFIG_HOME=$(mktemp -d)
XDG_DATA_HOME=$(mktemp -d)
export XDG_CONFIG_HOME XDG_DATA_HOME
printf '%s\n' "$PASSWORD" | "$BIN/farsight-passwd" --stdin user add "$USER_NAME"

USER=$USER_NAME DISPLAY=$DISPLAY_NUM "$BIN/farsight-agent" --listen lan --port "$PORT" --backend x11 --no-advertise &
AGENT=$!
for _ in $(seq 1 50); do (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break; sleep 0.2; done
# Paint after the agent is capturing, so the change also exercises damage tracking.
DISPLAY=$DISPLAY_NUM xsetroot -solid red

FARSIGHT_INTEROP_ADDR="127.0.0.1::$PORT" \
FARSIGHT_INTEROP_PASSWORD=$PASSWORD \
FARSIGHT_INTEROP_USERNAME=$USER_NAME \
FARSIGHT_INTEROP_EXPECT_ENCRYPTED=1 \
  cargo test -q -p farsight-net --test interop_tigervnc -- --nocapture
echo "agent end-to-end: OK"
