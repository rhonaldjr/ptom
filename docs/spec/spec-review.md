# Spec Review (2026-10-23)

Review of the original brief. Each finding lists the resolution and who resolved it.
**U** = decided by the user · **C** = resolved by the council ([session](../council/2026-10-23-initial-roadmap.md)) · **OPEN** = awaiting the user.

## Contradictions
| # | Finding | Resolution |
|---|---|---|
| 1 | Viewer platforms listed as "linux & macOS" in one place and "Linux, Windows" in another. | **U:** Linux and macOS only. This drops the Windows viewer and "Windows hooking" (SRV-10). [ADR-0004](../adr/0004-platform-scope.md) |
| 2 | "FLTK-based upstream" conflicts with a Rust implementation that prioritises COSMIC. | **U:** libcosmic/iced. [ADR-0003](../adr/0003-viewer-ui-libcosmic.md) |
| 3 | x0vncserver is described as sharing "X11 **and Wayland**", and w0vncserver also shares Wayland. | **C:** the x0 equivalent covers X11 only and the w0 equivalent covers Wayland. One `ptom-agent` binary with `--backend x11|wayland|headless`, plus compat aliases. |
| 4 | The README says "remote viewer… for Wayland based systems", while the spec says X11 and Wayland. | Spec wins. README updated (it also fixes a typo: "remove" → "remote"). |

## Ambiguities resolved
| # | Finding | Resolution |
|---|---|---|
| 5 | The "Virtual X server" (Xvnc) means writing an X server. | **U:** Smithay headless Wayland compositor plus Xwayland. [ADR-0006](../adr/0006-headless-smithay.md) |
| 6 | "Release each time it's committed" | **U:** every push to `main` produces a semver release. [ADR-0005](../adr/0005-release-per-push.md) |
| 7 | The persona list has 10 bullets, but "MITRE ATT&CK techniques, and business-logic abuse analysis" continues the Ethical Hacker bullet. | 9 personas. |
| 8 | "GnuTLS-equivalent in Rust" | rustls. [ADR-0007](../adr/0007-crypto-stack.md) |
| 9 | "Securely store and encrypt each client machine's information" doesn't say what is stored or where the key lives. | **C:** the OS keyring holds a vault key, and an AEAD-encrypted vault holds hosts, credentials, fingerprints and settings. [ADR-0008](../adr/0008-secret-storage.md) |
| 10 | "Better testing" is unspecified. | **C:** the Agile Testing Quadrants plan in CLAUDE.md, plus fuzzing and an interop matrix. |
| 11 | "Run with fewer privileges if you can" | **C:** user-level service, never root, systemd hardening, portals. [ADR-0009](../adr/0009-least-privilege.md) |

## Technical risks surfaced
| # | Risk | Mitigation |
|---|---|---|
| 12 | **Anonymous-TLS VeNCrypt types (TLSNone/TLSVnc/TLSPlain)** need anonymous Diffie-Hellman, which rustls does not implement and which is MITM-prone anyway. | **C:** not supported. The X509* types and RA2 cover encrypted interop with modern TigerVNC (1.13+). Supporting them would mean adding OpenSSL/GnuTLS. **Escalated as E2.** |
| 13 | **Wayland unattended access.** GNOME and KDE portals ask the local user for consent. Restore tokens help, but truly unattended access to a live Wayland session is restricted by design. | Headless sessions (SRV-1) cover unattended use. Live-session sharing documents the consent flow and uses COSMIC protocols where COSMIC allows them. |
| 14 | **Clean-room licensing.** TigerVNC is GPL-2.0 and ptom is MIT. Porting TigerVNC code would violate the licence. | [ADR-0011](../adr/0011-clean-room-licensing.md). Work from RFC 6143 and rfbproto. |
| 15 | **H.264 patents and licensing**, for both distribution and hardware encode paths. | Optional cargo feature using system VA-API / OpenH264 at runtime. Not in default builds until reviewed. |
| 16 | **System-key capture on Wayland** needs the compositor's keyboard-shortcuts-inhibit protocol, and support varies by DE. | Per-DE support matrix. Tracked in M2/M7. |
| 17 | **LAN discovery is also attacker reconnaissance (T1046) and enables rogue-agent spoofing.** | **C:** minimal TXT records. Discovered agents are untrusted until their fingerprint is verified. Advertising can be switched off. [ADR-0010](../adr/0010-lan-discovery.md) |
| 18 | **A web interface over Tailscale** means a WebSocket transport, an authenticated gateway and SSRF concerns. | Sans-I/O core now. The gateway is a Horizon item and needs its own council session. |
| 19 | **A release on every push** turns any bad merge into a published release. | Branch protection plus a mandatory CI gate. `main` is always releasable. |

## Questions put to the user (resolved 2026-10-23)
- **E1. Product name.** The README calls the app "Peepin Tom". The Privacy Analyst and Product Owner
  flag that "peeping Tom" means voyeur. On a remote-*viewing* tool, that may hurt distro inclusion,
  workplace adoption, and trust from the people being viewed. The binary name `ptom` is neutral either way.
  **Resolved by the user: renamed to Farsight** (binaries `farsight`, `farsight-agent`, `farsight-passwd`).
- **E2. Anonymous-TLS VeNCrypt.** Accept that ptom won't support it (council recommendation), or
  add an optional OpenSSL feature for maximum drop-in compatibility?
  **Resolved by the user: not supported** ([ADR-0007](../adr/0007-crypto-stack.md)).
