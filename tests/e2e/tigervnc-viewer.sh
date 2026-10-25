#!/usr/bin/env bash
# Interop (M9): TigerVNC's own vncviewer connects to the Farsight agent and
# shows its desktop. Runs in a Debian container; binaries from $BIN.
set -euo pipefail
cd "$(dirname "$0")/../.."
BIN=$(realpath "${BIN:-target/debug}")
docker build -q -t farsight-tigervnc-viewer tests/interop/tigervnc-viewer >/dev/null
docker run --rm -v "$BIN":/bin-farsight:ro -v "$PWD/tests/interop/tigervnc-viewer/check.sh":/check.sh:ro \
  farsight-tigervnc-viewer \
  dbus-run-session -- sh -c 'printf "\\n" | gnome-keyring-daemon --unlock --components=secrets >/dev/null && sh /check.sh' || {
  echo "::error title=viewer interop::TigerVNC viewer did not show the Farsight desktop"
  exit 1
}
