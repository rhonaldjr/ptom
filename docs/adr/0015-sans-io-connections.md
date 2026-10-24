# ADR-0015: Sans-I/O connection state machines with pluggable authenticators

- **Status:** Accepted · **Date:** 2026-10-23 · **Deciders:** Chief Architect (M1 implementation)
- **Requirements:** NET-1, NET-5, NET-6, SEC-1, PRD-9

## Context
The protocol core must run over plain TCP, TLS (VeNCrypt), RA2's AES-EAX framing and, later, WebSockets.
It must be unit-testable and fuzzable without sockets, and it must never interpret input before authentication.

## Decision
- `farsight-rfb` provides `ClientConnection` and `ServerConnection`. Callers `receive(bytes)`, call `poll()` for
  events, and send `take_output()`. There is no I/O, async or clock inside.
- The phase (version → security → auth → init → normal) is explicit, and bytes are interpreted only according to the
  current phase. Sending normal messages before the handshake completes is an `InvalidState` error.
- Security types implement `auth::Authenticator` (`advance(input, output) -> (consumed, progress)`). They live in
  `farsight-security`, and the caller chooses them per connection after applying `SecurityPolicy`.
- Security types that change the transport return `AuthProgress::UpgradeTransport(..)`. The state machine raises
  `UpgradeTransport`, pauses, and the caller wraps its transport (TLS, RA2 cipher) before calling `transport_upgraded()`.
  The state machine always sees plaintext.
- Rectangle payloads are decoded through the `client::RectDecoder` trait, implemented by `farsight-codec`. A decoder
  returns `Ok(None)` with **no side effects** until the full payload is available, so partial input is never applied.
- Every peer-announced length is checked against `Limits` before allocation or waiting. Oversized clipboard text is
  streamed past rather than buffered.

## Consequences
- The client and server machines are tested against each other in memory, and fuzzed directly.
- The future web gateway reuses the same machines over a WebSocket transport.
- Transport upgrades are the caller's job, so each adapter (TCP, TLS, WebSocket) composes the same way.
