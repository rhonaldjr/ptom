#!/bin/sh
# Starts Xvnc on :1 with a solid red root window so tests can check pixels.
set -e
SECURITY="${SECURITY:-VncAuth}"
mkdir -p /tmp/vnc
printf '%s\n' "${PASSWORD:-farsight}" | vncpasswd -f > /tmp/vnc/passwd
chmod 600 /tmp/vnc/passwd
Xvnc :1 -geometry 640x480 -depth 24 -rfbport 5901 -localhost=0 \
  -SecurityTypes "$SECURITY" -PasswordFile /tmp/vnc/passwd &
for _ in 1 2 3 4 5 6 7 8 9 10; do
  DISPLAY=:1 xsetroot -solid '#ff0000' 2>/dev/null && break
  sleep 0.5
done
wait
