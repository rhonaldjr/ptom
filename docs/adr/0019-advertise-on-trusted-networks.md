# ADR-0019: The agent announces itself only on trusted networks

- **Status:** Accepted · **Date:** 2026-10-24 · **Deciders:** user (P-32), after the Security Architect's review
- **Amends:** [ADR-0010](0010-lan-discovery.md) (when the agent advertises) · **Requirements:** PRD-4, VWR-13

## Context
ADR-0010 and the M6 council had the agent advertise `_rfb._tcp` whenever it listens off-loopback. On a laptop with
`--listen lan` that means announcing a remote-access service on every network it joins (cafés, hotels, offices),
which is free reconnaissance for anyone nearby (ATT&CK T1046).

## Decision
- The agent advertises **only on networks the user marked as theirs**, using the same network identity as the viewer's
  "This is my network" (ADR-0012: SHA-256 of gateway MAC + subnet).
- `farsight-agent --trust-this-network` marks the current network, and `--untrust-this-network` unmarks it. The list is
  `$XDG_CONFIG_HOME/farsight/agent/trusted-networks` (0600).
- The agent re-checks every 30 s. When the computer moves to an untrusted network, it withdraws the announcement (mDNS
  goodbye). When it comes back, it announces again.
- It still **accepts** connections on untrusted networks when `--listen lan` is set; it just doesn't announce. `--no-advertise`
  disables announcing everywhere.

## Consequences
At home, viewers find the agent automatically after one command. Elsewhere it stays quiet. The viewer and agent keep separate
trust lists (different processes, files and keys); a shared list is possible later.
