#!/usr/bin/env bash
# End-to-end (M9, U-11): the TigerVNC compatibility names work as symlinks.
# vncpasswd sets the VNC password; vncserver starts, lists and kills a
# headless desktop; x0vncserver shares an Xvfb display with TigerVNC options.
# Needs: Xvfb; binaries in $BIN (default target/debug); XDG_RUNTIME_DIR.
set -euo pipefail
cd "$(dirname "$0")/../.."
BIN=$(realpath "${BIN:-target/debug}")
LINKS=$(mktemp -d)
for name in vncpasswd:farsight-passwd vncserver:farsight-agent x0vncserver:farsight-agent; do
  ln -s "$BIN/${name#*:}" "$LINKS/${name%%:*}"
done
export PATH="$LINKS:$PATH"
XDG_CONFIG_HOME=$(mktemp -d); XDG_DATA_HOME=$(mktemp -d)
export XDG_CONFIG_HOME XDG_DATA_HOME
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-$(mktemp -d)}
banner() { timeout 5 bash -c "exec 3<>/dev/tcp/127.0.0.1/$1; head -c 12 <&3"; }

# Without a password vncserver must refuse (headless needs one).
if vncserver :5 >/dev/null 2>&1; then echo "FAIL: started without a password"; exit 1; fi
printf 'secret-pw\n' | "$BIN/farsight-passwd" --stdin user add "$USER" >/dev/null
if vncpasswd -f </dev/null 2>/dev/null; then echo "FAIL: vncpasswd -f should be refused"; exit 1; fi

vncserver :5 -geometry 800x600 -depth 24
vncserver -list | grep -q '^:5'
banner 5905 | grep -q 'RFB 003.008'
vncserver -kill :5
sleep 1
if banner 5905 2>/dev/null | grep -q RFB; then echo "FAIL: :5 still running"; exit 1; fi

Xvfb :96 -screen 0 320x200x24 -nolisten tcp >/dev/null 2>&1 &
XVFB=$!
trap 'kill $XVFB ${X0:-} 2>/dev/null || true' EXIT
sleep 1
x0vncserver -display :96 -rfbport 15996 -SecurityTypes VncAuth -AlwaysShared &
X0=$!
sleep 2
banner 15996 | grep -q 'RFB 003.008'
echo "compatibility names: OK"
