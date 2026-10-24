# ADR-0009: Least-privilege agent

- **Status:** Accepted (council) · **Date:** 2026-10-23
- **Requirements:** PRD-8, SRV-4, SRV-7, SEC-10

## Decision
- `farsight-agent` runs as the **desktop user** via a systemd **user** unit (`farsight-agent@.service`
  in `~/.config/systemd/user`, or shipped in `/usr/lib/systemd/user`). It never runs as root and refuses to start as UID 0.
- Hardening in units: `NoNewPrivileges=yes`, `ProtectSystem=strict`, `ProtectHome=read-only`
  (except XDG dirs), `PrivateTmp=yes`, `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`,
  `SystemCallFilter=@system-service`, `CapabilityBoundingSet=` (empty), `MemoryDenyWriteExecute=yes`.
- Wayland input and capture go through xdg-desktop-portal and libei, not `/dev/uinput`.
- Defaults: listen on **loopback only** with encrypted security types only. LAN exposure is an explicit opt-in.
- Future work (council item): split the network-facing protocol process from the capture/input
  process, with privilege separation over a Unix socket.

## Consequences
Some features that need root (login-screen/greeter access, system-wide sessions) are out of scope
until a separate, audited component is proposed to the council.
