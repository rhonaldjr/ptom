# ADR-0018: Agent architecture for sharing live desktops

- **Status:** Accepted · **Date:** 2026-10-24 · **Deciders:** user (U-2 to U-6), Chief Architect
- **Requirements:** SRV-2, SRV-3, SRV-4, SRV-7, SRV-8, SRV-9, SRV-10, SRV-12, SEC-10, NET-8

## Decision
- **Layers.**
  - `farsight-server` (use case, sans-I/O) holds `ServerSession` per viewer, a shared `Desktop` model with
    per-viewer damage tracking, `AccessControl` (lockout and connection limits), and the input/clipboard ports.
  - `farsight-codec` holds the encoders (Raw, ZRLE, Tight without JPEG). Every encoder round-trips through our own decoders.
  - Capture backends are adapters with one shared shape (`start` → `events` + `InputHandle`):
    - `farsight-capture-wayland`: xdg-desktop-portal RemoteDesktop/ScreenCast plus PipeWire.
    - `farsight-capture-x11`: x11rb with XDamage, XFixes, XTest and MIT-SHM.
  - `farsight-desktop` handles notifications and tray icons.
  - `apps/farsight-agent` is the composition root: it listens with tokio, does TLS (rustls) and RA2 framing exactly as
    the viewer does, and wires backend ↔ `Desktop` ↔ sessions.
- **Security offer.**
  - On loopback: encrypted types plus VncAuth (only if a VncAuth password is set).
  - On the LAN (`--listen lan`, U-5): encrypted types only, unless `--allow-legacy`.
  - The identity is a self-signed certificate (rcgen) and an RA2 key, both created on first run in
    `$XDG_CONFIG_HOME/farsight/agent/` (0700/0600). `farsight-agent --show-identity` prints the fingerprints viewers will see.
  - Credentials come from `farsight-passwd` (ADR-0014).
- **Access approval (U-2).** After authentication, a viewer signed in as the OS user running the agent is let in.
  Anyone else, including every password-only VncAuth login, triggers an Allow/Deny desktop notification for the
  person at the screen. It is denied after 30 s, or if notifications are unavailable.
- **Presence (SRV-12, U-4).** A tray icon (red while viewers are connected, with "Disconnect everyone") plus a
  notification for every connection and disconnection. Viewers can't turn this off.
- **Wayland consent (U-3).** The portal restore token is kept in `$XDG_CONFIG_HOME/farsight/agent/portal-token`, so
  approval is needed once per login. `farsight-agent --forget-portal-approval` deletes it.
- **Process.** A systemd **user** unit is shipped but not enabled (U-6), with ADR-0009 hardening. The agent refuses to
  run as root.

## Consequences
- Truly unattended access to a live Wayland session still depends on the desktop's portal honouring restore tokens.
  Headless desktops (M7) are the fully unattended path.
- X11 can't isolate clients from each other (any X client can inject input). This is documented as accepted in the threat model.
