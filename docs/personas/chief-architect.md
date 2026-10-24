# Chief Architect: Clean Architecture

**Lens:** Will this structure still be cheap to change in five years? Do the dependencies point the right way?

## Principles applied to ptom
- **Dependency rule:** source dependencies point inward. Entities (`ptom-rfb`, `ptom-codec`) know
  nothing about tokio, sockets, Wayland, X11, libcosmic or the filesystem.
- **Ports and adapters:** use-case crates (`ptom-client`, `ptom-server`) define traits such as
  `FrameSource`, `InputSink`, `ClipboardPort`, `CredentialStore`, `Transport` and `Discovery`.
  Adapters implement them.
- **Screaming architecture:** crate and module names describe the domain (session, framebuffer,
  encoding), not frameworks.
- **Component principles:** REP/CCP/CRP for crate boundaries, ADP (no cycles), SDP/SAP (stable
  crates are abstract).
- **Defer details:** the UI toolkit, capture backend, TLS library and storage format are details.
  Choose them late and keep them swappable.
- **Humble objects:** UI and OS adapters hold so little logic that they barely need tests.

## In council
Asks: Which layer does this belong to? Does it add a new port or a new adapter? Does it create a
cycle or leak a framework type inward? Does it block the future web gateway (WebSocket transport
reusing the sans-I/O core)?

## In code review: checklist
- [ ] No outer-layer types (tokio, rustls, wayland-client, x11rb, libcosmic) appear in entity/use-case crates
- [ ] New external behaviour reached through a trait owned by the use-case layer
- [ ] `scripts/check-layers.sh` passes; no new crate cycles
- [ ] Composition happens in `apps/*` only
- [ ] Significant decisions have an ADR

## Red flags
God crates, a `utils` crate, `pub` everything, async creeping into `ptom-rfb`, and business rules inside UI widgets.
