#!/usr/bin/env bash
# Renders the logos in dist/icons/*.svg into every size the desktops need.
# The output is committed, so builds don't need an SVG renderer; run this
# again after changing an SVG. Uses a throwaway Ubuntu container.
#   dist/icons/hicolor/<N>x<N>/apps/<app id>.png   app menu, dock, taskbar
#   dist/icons/tray/<name>-<N>.rgba                 tray pixmaps (raw RGBA)
#   dist/icons/Farsight.icns                        the macOS app bundle
set -euo pipefail
cd "$(dirname "$0")/../dist/icons"
exec docker run --rm -u 0 -v "$PWD":/i -w /i -e OWNER="$(id -u):$(id -g)" ubuntu:24.04 bash -euo pipefail -c '
  apt-get -qq update >/dev/null
  apt-get install -y -qq librsvg2-bin imagemagick icnsutils >/dev/null 2>&1
  for id in ca.akrotechnologies.Farsight ca.akrotechnologies.FarsightSharing; do
    for n in 16 22 24 32 48 64 128 256 512; do
      mkdir -p hicolor/${n}x${n}/apps
      rsvg-convert -w $n -h $n $id.svg -o hicolor/${n}x${n}/apps/$id.png
    done
  done
  # While someone is watching, the agent tray icon gets a red dot.
  sed "s#</svg>#  <circle cx=\"100\" cy=\"28\" r=\"22\" fill=\"\#e5243b\" stroke=\"\#ffffff\" stroke-width=\"6\"/>\n</svg>#" \
    ca.akrotechnologies.FarsightSharing.svg > /tmp/sharing-active.svg
  mkdir -p tray
  for n in 32 64; do
    rsvg-convert -w $n -h $n ca.akrotechnologies.Farsight.svg | convert png:- -depth 8 rgba:tray/viewer-$n.rgba
    rsvg-convert -w $n -h $n ca.akrotechnologies.FarsightSharing.svg | convert png:- -depth 8 rgba:tray/sharing-$n.rgba
    rsvg-convert -w $n -h $n /tmp/sharing-active.svg | convert png:- -depth 8 rgba:tray/sharing-active-$n.rgba
  done
  png2icns Farsight.icns $(for n in 16 32 48 128 256 512; do echo hicolor/${n}x${n}/apps/ca.akrotechnologies.Farsight.png; done) >/dev/null
  chown -R "$OWNER" hicolor tray Farsight.icns
'
