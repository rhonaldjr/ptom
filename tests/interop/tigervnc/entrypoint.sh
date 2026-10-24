#!/bin/sh
# Starts Xvnc on :1 with a solid red root window so tests can check pixels.
# SECURITY selects Xvnc's -SecurityTypes; a self-signed certificate and an
# RSA key are generated for the X509* and RA2* types.
set -e
SECURITY="${SECURITY:-VncAuth}"
mkdir -p /tmp/vnc
printf '%s\n' "${PASSWORD:-farsight}" | vncpasswd -f > /tmp/vnc/passwd
chmod 600 /tmp/vnc/passwd
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=farsight-interop" \
  -keyout /tmp/vnc/key.pem -out /tmp/vnc/cert.pem 2>/dev/null
openssl genrsa -out /tmp/vnc/rsa.pem 2048 2>/dev/null
EXTRA=""
if Xvnc -help 2>&1 | grep -q RSAKey; then EXTRA="-RSAKey /tmp/vnc/rsa.pem"; fi
Xvnc :1 -geometry 640x480 -depth 24 -rfbport 5901 -localhost=0 \
  -SecurityTypes "$SECURITY" -PasswordFile /tmp/vnc/passwd \
  -X509Cert /tmp/vnc/cert.pem -X509Key /tmp/vnc/key.pem $EXTRA &
for _ in 1 2 3 4 5 6 7 8 9 10; do
  DISPLAY=:1 xsetroot -solid '#ff0000' 2>/dev/null && break
  sleep 0.5
done
wait
