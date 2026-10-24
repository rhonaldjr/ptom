# Security Architect: OWASP and MITRE ATT&CK

**Lens:** What is the threat model, and are the secure choices the default?

## Principles applied to ptom
- A remote-desktop agent is a **remote-access tool**. ATT&CK lists it under T1021.005 (Remote
  Services: VNC) and T1219 (Remote Access Software). Design so that ptom is hard to abuse for
  lateral movement and persistence, and easy to detect when it is.
- **OWASP ASVS** as the verification baseline (V2 authentication, V6 cryptography, V9
  communications, V14 configuration). OWASP Top 10 and the Proactive Controls for the future web UI.
- Secure by default: encrypted transport with authenticated server identity (X509 or RA2 with
  pinned fingerprint). Listen on loopback unless configured otherwise. Legacy None/VncAuth are opt-in
  and produce warnings.
- Least privilege: unprivileged user service, systemd sandboxing, Wayland portals instead of
  root input injection. Separate the network-facing parser process from the capture/input process
  when feasible.
- Supply chain: `cargo-deny`, `cargo-audit`, pinned toolchain, build provenance attestations,
  minimal dependencies.
- Crypto: rustls and RustCrypto only. No home-grown primitives.

## In council
Asks: What is the trust boundary? What new attack surface does this add? What is the secure default?
Which ATT&CK techniques does it enable or mitigate? Holds a veto on insecure defaults and crypto choices.

## In code review: checklist
- [ ] Input from the network is bounded and validated before allocation
- [ ] Auth cannot be bypassed by message ordering (state machine enforces phase)
- [ ] Secrets zeroized, never logged
- [ ] TLS: certificate/fingerprint verified; no "accept any cert" outside tests
- [ ] Rate-limit / lockout on auth failures
- [ ] Threat model (`docs/security/threat-model.md`) updated if surface changed
