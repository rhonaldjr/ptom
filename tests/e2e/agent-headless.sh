#!/usr/bin/env bash
# End-to-end (Q4, M7): farsight-agent runs a headless desktop whose startup
# script opens a full-screen red terminal (foot); a Farsight viewer session
# connects over an encrypted security type, signs in as the owner, and
# decodes the red desktop. The headless compositor is the only display here.
# Needs: foot; binaries in $BIN (default target/debug); XDG_RUNTIME_DIR.
set -euo pipefail
cd "$(dirname "$0")/../.."
BIN=${BIN:-target/debug}
PORT=${PORT:-15991}
USER_NAME=farsight-e2e
PASSWORD=farsight-e2e-pw

XDG_CONFIG_HOME=$(mktemp -d)
XDG_DATA_HOME=$(mktemp -d)
export XDG_CONFIG_HOME XDG_DATA_HOME
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-$(mktemp -d)}
chmod 700 "$XDG_RUNTIME_DIR"
# Regression (M7 review, blocker): without a password a headless desktop must not start.
if "$BIN/farsight-agent" --backend headless --port "$PORT" --no-advertise 2>/dev/null; then
  echo "FAIL: a headless desktop started without a password"; exit 1
fi
printf '%s\n' "$PASSWORD" | "$BIN/farsight-passwd" --stdin user add "$USER_NAME"

mkdir -p "$XDG_CONFIG_HOME/foot"
printf '[colors]\nbackground=ff0000\nforeground=ff0000\n' > "$XDG_CONFIG_HOME/foot/foot.ini"
STARTUP=$XDG_CONFIG_HOME/startup
cat > "$STARTUP" <<'EOF'
#!/bin/sh
exec foot --fullscreen sleep 600
EOF
chmod 700 "$STARTUP"

AGENT=
cleanup() { [ -n "$AGENT" ] && kill "$AGENT" 2>/dev/null || true; }
trap cleanup EXIT
unset DISPLAY WAYLAND_DISPLAY
USER=$USER_NAME "$BIN/farsight-agent" --backend headless --geometry 640x480 --startup "$STARTUP" \
  --listen lan --port "$PORT" --no-advertise &
AGENT=$!
for _ in $(seq 1 50); do (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null && break; sleep 0.2; done

FARSIGHT_INTEROP_ADDR="127.0.0.1::$PORT" \
FARSIGHT_INTEROP_PASSWORD=$PASSWORD \
FARSIGHT_INTEROP_USERNAME=$USER_NAME \
FARSIGHT_INTEROP_EXPECT_ENCRYPTED=1 \
  cargo test -q -p farsight-net --test interop_tigervnc -- --nocapture
echo "headless agent end-to-end: OK"
