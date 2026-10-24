# Charter: Wayland capture and input through the portal (M4)

- **Quadrant:** Q3 (business-facing, critiques the product)
- **Crate:** `farsight-capture-wayland`
- **Status:** written 2026-10-24, not yet run. Run it on every release until M7 provides a CI compositor.

**Explore** the Wayland capture backend (xdg-desktop-portal RemoteDesktop + ScreenCast, PipeWire)
**with** a live Wayland session on Pop!_OS COSMIC, GNOME and KDE Plasma
**to discover** whether the permission flow, restore token, frames, cursor and injected input work
on each desktop, and how failures read to a person.

## Setup

| | COSMIC | GNOME | KDE |
|---|---|---|---|
| Distro | Pop!_OS 24.04 (COSMIC) | Pop!_OS or Ubuntu, GNOME on Wayland | Kubuntu/Fedora KDE, Plasma on Wayland |
| Portal backend | `xdg-desktop-portal-cosmic` | `xdg-desktop-portal-gnome` | `xdg-desktop-portal-kde` |

Before each session, record: distro and version, `echo $XDG_CURRENT_DESKTOP $XDG_SESSION_TYPE`,
`pipewire --version`, the portal backend's package version, and the monitor layout (count, scale).

Build tools: `libpipewire-0.3-dev` and `libclang-dev`, or use `scripts/docker-cargo.sh`. The smoke test
must **run** on the desktop being tested (it needs the session's D-Bus and PipeWire), so build on the
host or copy the test binary out of `target-docker/`.

## Steps

1. **First run, dialog.** In a terminal inside the Wayland session run
   `cargo test -p farsight-capture-wayland -- --ignored --nocapture live_portal_smoke`.
   Expect the desktop's screen-sharing dialog asking for a screen **and** remote control
   (keyboard and pointer). Choose one monitor and allow.
   Expect output listing `frame WxH` lines within 5 s, `restore token issued: true`, and
   (on COSMIC/GNOME/KDE with metadata cursors) at least one `cursor` line after moving the mouse.
   The pointer should jump to near the top-left corner (the test sends a move to 10,10).
2. **Decline.** Run again and press Cancel in the dialog. Expect the test to fail with
   "screen sharing was declined at the remote desktop", not a hang or a panic.
3. **Damage.** While step 1 runs, type in a window. Note whether `damage=Some(n)` (the compositor
   sends `SPA_META_VideoDamage`) or `damage=None` appears. Both are correct; record which.
4. **Frame rate.** Play a video or drag a window during step 1. Frames must not exceed about
   30 per second (count lines per second in the output).
5. **Stop sharing.** During step 1, stop sharing from the desktop's indicator (COSMIC panel,
   GNOME top-bar icon, KDE tray). Expect the test to end with
   "closed: screen sharing was stopped at the remote desktop" or "the screen stream ended".
6. **HiDPI.** With a monitor scaled to 200 %, repeat step 1 and check that the pointer lands at the
   right place when an integration (the agent, once wired) sends coordinates near the screen edges.
   Record the frame size and the portal's logical size.
7. **No RemoteDesktop portal.** On a desktop whose portal lacks RemoteDesktop (e.g. a wlroots
   session with only `xdg-desktop-portal-wlr`), run step 1. Expect an error that names the desktop
   (`XDG_CURRENT_DESKTOP`) and the missing RemoteDesktop portal.
8. **Restore token (U-3).** Once the agent stores the token, restart it in the same login session and
   confirm no dialog appears. Revoke the permission (desktop settings, or the agent's settings once
   built) and confirm the dialog returns.

## Record

For each desktop: what you did, what happened, screenshots of any dialog or error, and the issue
number for each bug filed. Note anything confusing in the dialog wording, since the person at the
screen sees it, not the remote viewer.
