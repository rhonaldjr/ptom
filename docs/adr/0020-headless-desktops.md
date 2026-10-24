# ADR-0020: Headless desktops in practice

- **Status:** Accepted · **Date:** 2026-10-24 · **Deciders:** user (U-9, U-10), Chief Architect, Security Architect
- **Builds on:** [ADR-0006](0006-headless-smithay.md) (Smithay headless compositor + Xwayland) · **Requirements:** SRV-1, SRV-5, VWR-4, NET-8

## Context
ADR-0006 chose an embedded Smithay compositor for persistent desktops (TigerVNC's `vncserver` / `Xvnc`). Building it
(M7) forced a few concrete choices that ADR-0006 left open.

## Decision
1. **Crate.** `farsight-headless` (adapter layer) runs Smithay 0.7 on its own thread with a calloop event loop. It only
   uses safe Rust: the Wayland display is dispatched through a duplicated poll fd, not Smithay's `unsafe` pattern.
2. **Rendering.** Software only (pixman) into one off-screen image per screen; no GPU, DRM, libinput or seat access.
   Frames go to the agent with damage rectangles. GPU paths are M10.
3. **Screens.** Each screen of a `SetDesktopSize` layout is a virtual monitor (`wl_output`); layouts are validated
   (≤ 16 screens, inside the desktop, no overlaps, ≤ 8192 px a side).
4. **Window management.** Deliberately simple: floating windows, new ones cascaded, click to focus and raise,
   client-requested moves, maximise and full screen. No interactive resizing yet. New Wayland windows get the keyboard
   after their first frame (foot 1.11 crashes otherwise).
5. **X11 apps.** Xwayland, rootless, started by Smithay. Smithay 0.7 only links X11 windows to surfaces through the
   `xwayland_shell_v1` protocol, so **X11 apps need Xwayland 23.1+** (Ubuntu / Pop!_OS 24.04 and newer). With an older
   Xwayland the desktop starts without X11 support and says so. Xwayland gets no authority file, so the agent runs
   with umask 077 (the session gets the user's umask back), the socket is made owner-only (0700, never through a
   symlink), no abstract socket is opened, and `/tmp/.X11-unix` must be owned by root or the user and sticky.
6. **Session.** The user's `~/.config/farsight/headless-startup` (created with a terminal + file manager default)
   runs under `dbus-run-session`, in its own process group, with `WAYLAND_DISPLAY` and `DISPLAY` set (U-10).
7. **Shared input (U-9).** `farsight-server::Control`: the first viewer to send input controls; others watch. Control
   passes when the controller disconnects or has been idle for 10 s, and the owner can always take over. Clipboard
   from a viewer and resizing count as input; the desktop's clipboard goes only to the controller (or the owner).
   Keys and buttons still held are released when control changes hands. RFB has no "hand over" message, hence
   the idle rule. Resizes are limited to one per 250 ms and 33.5 million pixels.
8. **Others watching.** With two or more viewers, the compositor draws a 4 px red frame around every screen, so
   everyone inside the desktop knows (privacy condition from the M7 council).
9. **No approval prompt, so a password is mandatory.** Nobody sits at a headless desktop. The agent refuses to start
   a headless desktop without an agent user or VNC password and never offers security type None, even on
   loopback (persona review 2026-10-24).
10. **systemd.** `farsight-headless@<port>.service` and `.socket` (user units, loopback by default). The instance is
    the port; the agent derives display `:N` from port `5900+N`. The service has light hardening because it is the
    parent of a whole desktop session (apps need the home folder, setuid helpers and JIT).

## Consequences
- Persistent desktops work today on any Linux with Wayland apps; X11 apps need a 24.04-era Xwayland.
- CI runs the compositor tests and an end-to-end test (agent + foot + viewer) on Ubuntu 24.04.
- Interactive resizing, server-side decorations and GPU rendering remain to do.
