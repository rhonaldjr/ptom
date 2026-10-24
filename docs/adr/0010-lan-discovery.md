# ADR-0010: LAN discovery over mDNS / DNS-SD

- **Status:** Accepted (council), extended by [ADR-0012](0012-active-rfb-probing.md) · **Date:** 2026-10-23
- **Requirements:** PRD-4, VWR-13

## Decision
- Agents advertise `_rfb._tcp` (interoperable with other VNC browsers) with a minimal TXT record:
  `farsight=1`, protocol version, and the **TLS/RA2 key fingerprint**. No usernames, OS versions or
  DE names. The instance name defaults to the hostname and can be configured.
- Advertising is **on only when the agent listens off-loopback** and can be disabled (`--no-advertise`).
- The viewer lists discovered agents as **unverified** until the user confirms the fingerprint
  on first connect (TOFU), or it matches a pinned fingerprint. This guards against rogue-agent spoofing.
- Implementation: a pure-Rust mDNS crate (e.g. `mdns-sd`) in `farsight-discovery`. Avahi is not required.

## Consequences
Discovery works without configuration on home networks and leaks only presence plus a fingerprint.
