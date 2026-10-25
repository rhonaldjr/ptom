#!/bin/sh
# Starts TightVNC's Xvnc on :1 (VncAuth, password "farsight") with a solid
# red root window so tests can check pixels.
set -e
mkdir -p /tmp/vnc
printf '%s\n' "${PASSWORD:-farsight}" | tightvncpasswd -f > /tmp/vnc/passwd
chmod 600 /tmp/vnc/passwd
Xtightvnc :1 -geometry 640x480 -depth 24 -rfbport 5901 -rfbauth /tmp/vnc/passwd \
  -co /etc/X11/rgb -fp /usr/share/fonts/X11/misc/ &
for _ in 1 2 3 4 5 6 7 8 9 10; do
  DISPLAY=:1 xsetroot -solid '#ff0000' 2>/dev/null && break
  sleep 0.5
done
wait
