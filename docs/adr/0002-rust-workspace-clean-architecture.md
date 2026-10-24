# ADR-0002: Rust workspace layered by Clean Architecture

- **Status:** Accepted · **Date:** 2026-10-23 · **Deciders:** user, Chief Architect
- **Requirements:** PRD-1, PRD-6, PRD-8, PRD-9, NET-6

## Context
Farsight has one protocol core and several faces: viewer, agent, password tool, and later a web
gateway. It also has many interchangeable details: X11, Wayland portals, headless Smithay,
rustls, libcosmic, mDNS. A memory-safe language is a security requirement for code that parses
untrusted network input.

## Decision
- Rust (edition 2024), in a single Cargo workspace. `unsafe_code = "forbid"` is set workspace-wide.
- The layers, from innermost to outermost:
  1. **Entities:** `farsight-rfb` (wire types, codec, sans-I/O state machines), `farsight-codec` (encodings).
  2. **Use cases:** `farsight-security` (negotiation and auth logic), `farsight-client`, `farsight-server`. These own the **ports** (traits).
  3. **Adapters:** `farsight-net`, `farsight-vault`, `farsight-discovery`, and later `farsight-capture-*` and `farsight-headless`.
  4. **Frameworks/drivers:** `apps/farsight`, `apps/farsight-agent`, `apps/farsight-passwd`. These are composition roots only.
- Dependencies point inward only. `scripts/check-layers.sh` enforces this in CI.
- Protocol logic is **sans-I/O**: it takes bytes and events in and returns bytes and actions out.
  This lets the same core run over TCP, TLS, or the future WebSocket gateway, and makes it unit-testable without sockets.

## Consequences
- More crates up front. Each one has a single reason to change.
- The async runtime (tokio) is confined to adapters and apps.
- Fuzzing targets the entity crates directly.

## Alternatives considered
- A single crate. Simpler, but the dependency rule could not be enforced and builds are slower to iterate.
- Async-first core. Couples the protocol to tokio and complicates testing.
