# Compatibility matrix (M9)

What is checked automatically on every change (CI) and what still needs a person at a real desktop. A row is
**certified** once its hand check is recorded here with a date and version.

## VNC software (automated)

| Other software | Direction | Security | How | Status |
|---|---|---|---|---|
| TigerVNC 1.12 server (Debian bookworm) | Farsight viewer → server | VncAuth, None | CI `interop` | ✅ automated |
| TigerVNC 1.15 server (Debian trixie) | Farsight viewer → server | X509Vnc, RA2, RA2_256 | CI `interop` | ✅ automated |
| TightVNC 1.3 server | Farsight viewer → server | VncAuth | CI `interop (tightvnc)` | ✅ automated |
| TigerVNC 1.15 viewer | TigerVNC viewer → Farsight agent | VncAuth (loopback) | CI `viewer-interop` | ✅ automated |
| RealVNC Viewer | RealVNC → Farsight agent | — | Proprietary; can't run in CI (P-48) | ⬜ hand check |
| noVNC | browser → Farsight agent | — | Needs a WebSocket gateway (Horizon item; P-48) | ⬜ not yet possible |

## Desktops (hand checks)

Charter for each row: connect with the Farsight viewer; check tabs, tray icon, clipboard both ways, full screen,
keyboard (including a non-US layout through physical keys) and the F8 menu's special keys; share the desktop with
the agent and connect from another computer; check the presence icon and notifications.

| Desktop | Viewer | Agent (live) | Agent (headless) | Last checked |
|---|---|---|---|---|
| COSMIC (Pop!_OS 24.04) | ⬜ | ⬜ (Wayland portal) | ✅ automated (CI `headless`) | — |
| GNOME (Pop!_OS 22.04) | ⬜ | ⬜ | ✅ automated | — |
| KDE Plasma | ⬜ certification pending | ⬜ certification pending | ✅ automated | — |
| Xfce (X11) | ⬜ certification pending | ✅ automated (X11 via Xvfb, CI `agent-e2e`) | ✅ automated | — |
| macOS (Apple Silicon, viewer only) | ⬜ builds in CI; polish pending (P-49) | — | — | — |
