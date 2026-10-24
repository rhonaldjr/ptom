#!/usr/bin/env bash
# Builds the two Debian packages from already-compiled release binaries:
#   farsight        the viewer
#   farsight-agent  the agent (server), farsight-passwd and the systemd user unit
# Usage: scripts/build-deb.sh <version> <binary-dir> <out-dir>
# Needs dpkg-dev (dpkg-deb, dpkg-shlibdeps). Library dependencies are computed
# from the binaries, so the packages install on the release they were built on
# (Ubuntu 22.04 / Pop!_OS 22.04) and anything newer.
set -euo pipefail
version="$1" bins="$(realpath "$2")" out="$(realpath -m "$3")"
arch="$(dpkg --print-architecture)"
cd "$(dirname "$0")/.."
root="$PWD"
mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# package <name> <summary> <description> <extra-depends> <bin>...
package() {
  local name="$1" summary="$2" description="$3" extra="$4"
  shift 4
  local dir="$work/$name"
  install -d "$dir/DEBIAN" "$dir/usr/bin" "$dir/usr/share/doc/$name"
  for bin in "$@"; do install -m 0755 "$bins/$bin" "$dir/usr/bin/$bin"; done
  install -m 0644 "$root/LICENSE" "$dir/usr/share/doc/$name/copyright"
  install -m 0644 "$root/README.md" "$dir/usr/share/doc/$name/README.md"
  case "$name" in
    farsight)
      install -d "$dir/usr/share/applications"
      cat > "$dir/usr/share/applications/farsight.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Farsight
GenericName=Remote Desktop Viewer
Comment=View and control other computers
Exec=farsight
Icon=video-display
Terminal=false
Categories=Network;RemoteAccess;
Keywords=VNC;remote;desktop;viewer;
EOF
      ;;
    farsight-agent)
      install -d "$dir/usr/lib/systemd/user"
      install -m 0644 "$root/dist/systemd/farsight-agent.service" "$root/dist/systemd/farsight-headless@.service" \
        "$root/dist/systemd/farsight-headless@.socket" "$dir/usr/lib/systemd/user/"
      ;;
  esac

  # dpkg-shlibdeps wants a debian/control next to it; give it a throwaway one.
  local depends executables=()
  for bin in "$@"; do executables+=("-e$dir/usr/bin/$bin"); done
  depends="$(cd "$work" && mkdir -p debian && printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' "$name" "$name" > debian/control &&
    dpkg-shlibdeps -O "${executables[@]}" | sed -n 's/^shlibs:Depends=//p')"
  [ -n "$extra" ] && depends="${depends:+$depends, }$extra"
  local recommends=""
  # Headless desktops: X11 apps (Xwayland 23.1+), a terminal, own D-Bus per desktop.
  [ "$name" = farsight-agent ] && recommends="xwayland, libegl1, dbus-user-session | dbus-x11, foot | xterm"

  cat > "$dir/DEBIAN/control" <<EOF
Package: $name
Version: $version
Architecture: $arch
Maintainer: Farsight developers <https://github.com/rhonaldjr/ptom>
Homepage: https://github.com/rhonaldjr/ptom
Section: net
Priority: optional
Depends: $depends
${recommends:+Recommends: $recommends
}Description: $summary
 $description
EOF
  dpkg-deb --root-owner-group --build "$dir" "$out/${name}_${version}_${arch}.deb" >/dev/null
  echo "$out/${name}_${version}_${arch}.deb"
}

package farsight "Farsight remote desktop viewer (VNC)" \
  "Connects to VNC servers, including TigerVNC and the Farsight agent, with tabs, discovery and an encrypted vault." \
  "libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-egl1, libvulkan1 | libegl1, fonts-dejavu-core | fonts-noto-core" farsight
package farsight-agent "Farsight desktop sharing agent (VNC server)" \
  "Shares your live X11 or Wayland desktop over VNC, encrypted, with approval prompts. Replaces x0vncserver and vncpasswd." \
  "xdg-desktop-portal, xkb-data" farsight-agent farsight-passwd
