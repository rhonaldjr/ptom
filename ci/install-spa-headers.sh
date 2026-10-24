#!/usr/bin/env bash
# Replaces the SPA headers of Ubuntu 22.04's PipeWire 0.3.48 with those of PipeWire 1.0.5.
#
# Why: the `pipewire`/`libspa` crates (0.8 and later) do not compile against 0.3.48's SPA
# headers (spa_video_info_raw lacks `flags`, `modifier` is signed, and spa_meta_first /
# spa_meta_region_is_valid are macros rather than functions). SPA is header-only, so newer
# headers change nothing at link time: we still link Ubuntu's libpipewire-0.3.so.0 and use
# only symbols it exports, so the binary runs against PipeWire 0.3.48 and later.
#
# The tarball is Ubuntu's immutable source upload, pinned by the SHA-256 listed in
# pipewire_1.0.5-1.dsc. Used by ci/builder.Dockerfile and the GitHub workflows. Needs root.
set -euo pipefail

VERSION=1.0.5
SHA256=d5e5f3d0b8460e5711c1571c500156fd61768ba55a062eaeb94356abdb955a56
URL="http://archive.ubuntu.com/ubuntu/pool/main/p/pipewire/pipewire_${VERSION}.orig.tar.bz2"
DEST=/usr/include/spa-0.2

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -sSfL -o "$work/pw.tar.bz2" "$URL"
echo "$SHA256  $work/pw.tar.bz2" | sha256sum -c --quiet -
tar -xjf "$work/pw.tar.bz2" -C "$work" "pipewire-${VERSION}/spa/include/spa"
rm -rf "${DEST:?}/spa"
mkdir -p "$DEST"
cp -r "$work/pipewire-${VERSION}/spa/include/spa" "$DEST/spa"
echo "installed SPA headers from PipeWire ${VERSION} into ${DEST}"
