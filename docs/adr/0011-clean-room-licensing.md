# ADR-0011: Clean-room implementation under MIT

- **Status:** Accepted (council) · **Date:** 2026-10-23
- **Requirements:** PRD-1

## Context
ptom is MIT-licensed. TigerVNC, libvncserver, TightVNC and x11vnc are GPL.

## Decision
- Implement from public specifications only: RFC 6143, the community RFB spec (rfbproto), RFC 8446 (TLS 1.3), and the relevant Wayland/portal protocol specs.
- Contributors (human or AI) must not copy, translate or closely paraphrase GPL VNC source.
  *Observing behaviour* of those programs as black boxes (interop testing) is fine.
- All dependencies must use licences compatible with MIT distribution. `deny.toml` enforces an allow-list.

## Consequences
Some edge-case behaviour must be discovered through interop testing rather than by reading upstream code.
