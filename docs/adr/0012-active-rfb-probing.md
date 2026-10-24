# ADR-0012: Discovery of any RFB server, including opt-in active probing

- **Status:** Accepted (council) · **Date:** 2026-10-23 · **Extends:** [ADR-0010](0010-lan-discovery.md)
- **Council:** [tray and discovery](../council/2026-10-23-tray-and-discovery.md#b-discover-any-vnc-server-not-just-farsight-agents)
- **Requirements:** PRD-4, VWR-16, VWR-17

## Context
Most VNC servers (TigerVNC, x11vnc, QEMU/libvirt) don't advertise over mDNS. The user wants the viewer to
find *every* server on their network automatically. Active probing is network service discovery
(ATT&CK T1046) and is inappropriate on networks the user doesn't own.

## Decision
- The `Discovery` port (in `farsight-client`) has two adapters, both feeding one `DiscoveredServer` entity
  that records its provenance (`Mdns` or `Probe`) and is always **unverified** until TOFU:
  1. **mDNS browser** (`_rfb._tcp`, any vendor). Always on.
  2. **Active prober.** Runs only on networks the user has marked "This is my network". Networks are
     identified by gateway MAC plus subnet, and the trusted list is stored in `farsight-vault` and can be cleared.
- Probe scope is a `ProbeScope` type. It covers directly attached RFC 1918 / link-local IPv4 subnets
  (at most /22 per interface) and the ARP/NDP neighbour table (which also covers IPv6). Ports default to
  5900–5910 and are configurable. Limits: at most 32 concurrent connects, 500 ms connect timeout. Each probe
  reads the `ProtocolVersion` banner and the security-type list using the existing `farsight-rfb` decoders, then
  closes. **No auth messages and no credentials are ever sent.**
- Results stay in memory. Clicking "Add" creates a saved session in the vault. It does not connect.
- Saved credentials are bound to a verified server identity, never to an IP or name.
- The web gateway (Horizon) composes the mDNS adapter only.

## Consequences
- Third-party servers are found automatically on home networks after one consent.
- The prober is a separately swappable adapter, and builds can omit it.
- The threat model gains the probing and spoofing rows.
