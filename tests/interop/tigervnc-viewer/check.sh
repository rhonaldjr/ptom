#!/bin/sh
# Inside the container: the Farsight agent runs a headless desktop (no
# approval prompt; its plain background is RGB 33,38,48); TigerVNC's
# vncviewer shows it full screen on an Xvfb (:98), whose screen must then
# show that background.
set -eu
BIN=/bin-farsight
export XDG_CONFIG_HOME=$(mktemp -d) XDG_DATA_HOME=$(mktemp -d) XDG_RUNTIME_DIR=$(mktemp -d)
Xvfb :98 -screen 0 640x480x24 -nolisten tcp >/dev/null 2>&1 &
sleep 1
printf 'interop-pw\n' | "$BIN/farsight-passwd" --stdin vncauth set >/dev/null
# Loopback: the agent allows VncAuth there (ADR-0018).
"$BIN/farsight-agent" --backend headless --geometry 600x400 --port 15997 --no-advertise >/tmp/agent.log 2>&1 &
sleep 2
printf 'interop-pw\n' | vncpasswd -f > /tmp/viewer-passwd
DISPLAY=:98 vncviewer -SecurityTypes VncAuth -passwd /tmp/viewer-passwd 127.0.0.1::15997 \
  >/tmp/viewer.log 2>&1 &
sleep 6
cat /tmp/viewer.log >&2
if ! xwd -display :98 -root -silent > /tmp/screen.xwd; then
  cat /tmp/viewer.log /tmp/agent.log
  exit 1
fi
python3 - <<'PY'
import struct, sys
data = open('/tmp/screen.xwd', 'rb').read()
header = struct.unpack('>25I', data[:100])
size, ncolors, width, height, bpl = header[0], header[19], header[4], header[5], header[12]
pixels = data[size + ncolors * 12:]
x, y = width // 2, height // 2
p = pixels[y * bpl + x * 4: y * bpl + x * 4 + 4]
red = (p[2], p[1], p[0]) if header[7] == 0 else (p[1], p[2], p[3])
print('viewer centre pixel', red)
from collections import Counter
c = Counter()
for yy in range(0, height, 4):
    for xx in range(0, width, 4):
        q = pixels[yy * bpl + xx * 4: yy * bpl + xx * 4 + 4]
        c[(q[2], q[1], q[0])] += 1
print('colours', c.most_common(5), 'byte order', header[7], 'size', width, height)
ok = red == (33, 38, 48)
if not ok:
    print(open("/tmp/viewer.log").read()[-1500:]); print(open("/tmp/agent.log").read()[-1500:])
sys.exit(0 if ok else 1)
PY
echo "TigerVNC viewer -> Farsight agent: OK"
