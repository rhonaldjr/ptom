# TigerVNC's viewer, for testing it against the Farsight agent (black-box; ADR-0011).
FROM debian:trixie-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends tigervnc-viewer tigervnc-tools xvfb x11-apps x11-xserver-utils \
      python3 libpixman-1-0 libxkbcommon0 xkb-data ca-certificates dbus gnome-keyring \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -m tester
USER tester
