# Council: Initial roadmap (M0–M10, Horizon)

Date: 2026-10-23 · Chair: Product Owner · Opened by: Business Analyst
Conducted by Claude, adopting each persona's framework in turn. The user can overrule any outcome.

Abbreviations: **CA** Chief Architect · **SE** Senior Engineer · **PO** Product Owner · **QA** QA Analyst ·
**SA** Security Architect · **UX** UX Designer · **PA** Privacy Analyst · **EH** Ethical Hacker · **BA** Business Analyst

---

## M0: Foundation
**Framing (BA):** Activity 7 "Operate & maintain" › walking skeleton. Outcome: every push to main yields a tested, attested release.

**Round 1**
| Persona | Position | Note |
|---|---|---|
| CA | SUPPORT | Layered workspace and a layer-check script make the dependency rule enforceable from day one. |
| SE | SUPPORT | Workspace lints (`unsafe_code = forbid`, clippy `unwrap_used`) set the bar early. |
| PO | SUPPORT | — |
| QA | CONCERN | A release on every push needs the full CI gate *inside* the release workflow, not just on PRs. **Condition accepted.** |
| SA | CONCERN | Supply chain: `cargo-deny`, build provenance attestation, least-privilege workflow tokens. **Condition accepted.** |
| UX | SUPPORT | — |
| PA | SUPPORT | — |
| EH | CONCERN | GitHub Actions pinned by tag can be hijacked (tj-actions, 2025). Pin to SHAs. **Accepted as follow-up**: Dependabot is configured now, and SHA pinning happens before the first public announcement. |
| BA | SUPPORT | — |

**Outcome:** ACCEPTED-WITH-CONDITIONS · Roadmap M0

---

## M1: RFB core
**Framing (BA):** Slice 1. Stories: handshake and message exchange in a sans-I/O core. AC: recorded TigerVNC transcripts replay to the expected state, and decoders never panic under fuzzing.

**Round 1**
| Persona | Position | Note |
|---|---|---|
| CA | SUPPORT | Sans-I/O lets the future web gateway reuse the core. |
| SE | SUPPORT | — |
| PO | SUPPORT | Required for everything else. |
| QA | CONCERN | Needs fuzz targets plus proptest round-trips per message. **Accepted.** |
| SA | **OBJECT** | None and VncAuth (DES, 8-char passwords) violate ASVS V2/V6. They should not ship at all. |
| UX | SUPPORT | — |
| PA | SUPPORT | — |
| EH | CONCERN | The state machine must reject input messages before auth completes (T1021.005 abuse). **Accepted.** |
| BA | SUPPORT | — |

**Round 2**
- **PO:** Drop-in replacement is the product promise. Most existing TigerVNC/QEMU/libvirt setups use VncAuth, and dropping it breaks PRD-1.
- **SA:** AMEND. Implement them but gate them: allowed by default on loopback only, opt-in elsewhere,
  with a log warning and a viewer warning, and never selected when an encrypted type is mutually available.
- **PO:** Accepts. **SA:** WITHDRAW, on that amendment.

**Outcome:** ACCEPTED-WITH-CONDITIONS (gated legacy auth, fuzz/proptest, phase enforcement) · Roadmap M1

---

## M2: Viewer MVP
**Framing (BA):** Slice 2 "View my existing VNC server". This is the first user-visible value.

**Round 1**
| Persona | Position | Note |
|---|---|---|
| CA | SUPPORT | The UI stays a humble object over `ptom-client`. |
| SE | CONCERN | Decoding five encodings is a lot for an MVP. |
| PO | SUPPORT | A viewer works against servers people already run, which is the fastest path to feedback. |
| QA | CONCERN | Needs a TigerVNC server container in CI for interop. **Accepted.** |
| SA | **OBJECT** | Shipping a viewer before M3 encourages plaintext connections. |
| UX | CONCERN | The encryption state must always be visible (heuristic #1), and a fullscreen escape must always work (#3). **Accepted.** |
| PA | SUPPORT | — |
| EH | CONCERN | A malicious server attacks the viewer (API10): oversized rects, cursor bombs, zlib bombs. Every decoder must be fuzzed. **Accepted.** |
| BA | SUPPORT | — |

**Round 2**
- **PO:** Real servers default to Tight/ZRLE, and Raw over Wi-Fi is unusable. SE's concern would cripple the slice.
- **SE:** AMEND. Accept Raw, CopyRect, Hextile, ZRLE and Tight *decoders only*. Encoders wait for M4/M8. WITHDRAW.
- **SA → PO:** Make M3 immediately follow M2, and the viewer must show "Not encrypted" in red with a
  one-time explanation before the first plaintext connection.
- **PO:** Accepts. **UX:** supports this (heuristic #5, error prevention). **SA:** WITHDRAW.

**Outcome:** ACCEPTED-WITH-CONDITIONS · Roadmap M2 (M3 locked as next)

---

## M3: Secure transport & vault
**Framing (BA):** Slice 2. Stories: verify identity, and remember credentials securely.

**Round 1**
| Persona | Position | Note |
|---|---|---|
| CA | SUPPORT | Vault is an adapter behind a `CredentialStore` port. |
| SE | SUPPORT | — |
| PO | SUPPORT | — |
| QA | CONCERN | A downgrade-attack test (MITM strips VeNCrypt from the type list) is required. **Accepted.** |
| SA | SUPPORT | rustls, RA2 and X509. Anonymous TLS is excluded (ADR-0007). |
| UX | CONCERN | The "identity changed" message must be human-readable and must not train people to click through (#9). **Accepted.** |
| PA | CONCERN | History must be clearable and the vault must be encrypted at rest. **Accepted** (ADR-0008). |
| EH | CONCERN | Fingerprint fatigue: a changed fingerprint must be harder to accept than a first-seen one. **Accepted.** |
| BA | SUPPORT | — |

**Debate on anonymous TLS (sub-item SEC-5a)**
- **R1 PO:** OBJECT to dropping TLSVnc. Some TigerVNC clients are configured with only it.
- **R2 SA:** rustls can't do anonymous DH, and anonymous TLS gives no server authentication. Adding
  OpenSSL contradicts ADR-0007's supply-chain stance. **CA:** an optional feature behind an adapter is
  possible, but it adds a second TLS stack. **PO:** MAINTAIN, because the cost lands on migrating users.
- **R3 Chair synthesis:** exclude it now, document the migration (enable X509Vnc/RA2 on clients), and
  revisit if users report it. **SA** accepts. **PO** accepts the synthesis *but asks for the user's confirmation*,
  because it touches the drop-in promise.

**Outcome:** M3 ACCEPTED-WITH-CONDITIONS · SEC-5a **ESCALATED → E2**

---

## M4: COSMIC / Wayland agent
**Framing (BA):** Slice 3 "Reach my Pop!_OS COSMIC desktop from the couch".

**Round 1**
| Persona | Position | Note |
|---|---|---|
| CA | SUPPORT | Capture and input are adapters behind `FrameSource`/`InputSink`, so X11 and headless later reuse the use cases. |
| SE | SUPPORT | — |
| PO | SUPPORT | COSMIC is the top priority, so it comes ahead of X11 even though X11 is easier. |
| QA | **OBJECT** | Portal-based Wayland capture can't run in hosted CI. Shipping without automated end-to-end coverage breaks the DoD. |
| SA | CONCERN | No off-loopback listening until M3 lands. Systemd hardening per ADR-0009. **Accepted.** |
| UX | CONCERN | The portal consent dialog is a flow ptom can't style, so it needs explanatory first-run copy. **Accepted.** |
| PA | **OBJECT** | The person at the screen must always know they are being viewed (PbD principles 6 and 7). |
| EH | CONCERN | View-only clients must be unable to send input or resize (API3). Accept/reject prompts must time out to *reject* (API6). **Accepted.** |
| BA | SUPPORT | — |

**Round 2**
- **CA → QA:** use cases are fully testable via fake ports in CI (Q1/Q2). Only the thin portal adapter is untested end-to-end.
- **QA:** AMEND. Accept, if every release runs a recorded Q3 exploratory charter on real Pop!_OS COSMIC until M7's
  headless compositor can host CI end-to-end tests. WITHDRAW.
- **PO → PA:** add SRV-12, a session-presence indicator plus "disconnect all", for the local user.
- **PA:** WITHDRAW, on the condition that the indicator cannot be disabled from the remote side.

**Outcome:** ACCEPTED-WITH-CONDITIONS · Roadmap M4 (depends on M3)

---

## M5: X11 agent
**Round 1:** CA SUPPORT (new adapter only) · SE SUPPORT · PO SUPPORT (Pop!_OS GNOME on X11, TigerVNC migrants) ·
QA SUPPORT (Xvfb makes end-to-end CI easy) · SA CONCERN: XTest lets any X client inject input, so document that
X11 is inherently less isolated. **Accepted.** · UX/PA/EH/BA SUPPORT.
**Outcome:** ACCEPTED-WITH-CONDITIONS · Roadmap M5

---

## M6: Multi-session & discovery
**Framing (BA):** Slice 5 "All my machines in one place".

**Round 1**
| Persona | Position | Note |
|---|---|---|
| CA | SUPPORT | `Discovery` port, mDNS adapter. |
| SE | SUPPORT | — |
| PO | SUPPORT | A headline differentiator over TigerVNC. |
| QA | SUPPORT | — |
| SA | CONCERN | Advertising helps attackers find targets (T1046). |
| UX | SUPPORT | Recognition over recall (#6). Agents are labelled "Computers on your network" (#2). |
| PA | **OBJECT** | Broadcasting by default violates *privacy as the default*. |
| EH | **OBJECT** | Rogue agents can spoof names ("Office-PC") to harvest passwords. |
| BA | SUPPORT | — |

**Round 2**
- **PO:** Discovery that's off by default doesn't deliver PRD-4.
- **SA (amend):** advertise only when the agent already listens off-loopback, which means the user has
  already chosen LAN exposure. `--no-advertise` turns it off.
- **PA:** AMEND. The TXT record carries only presence plus the key fingerprint: no username, OS or DE. Then WITHDRAW.
- **EH:** MAINTAIN until spoofing is addressed.

**Round 3**
- **Chair synthesis:** discovered agents are shown as *unverified* and credentials are never sent until the
  TLS/RA2 identity is verified (TOFU, with the fingerprint shown from the TXT record and the handshake). A spoof test joins the AC.
- **EH:** accepts.

**Outcome:** ACCEPTED-WITH-CONDITIONS (ADR-0010) · Roadmap M6

---

## M7: Headless desktops
**Round 1:** CA SUPPORT (ADR-0006) · SE CONCERN: compositor work is large, so slice it as single monitor first, then
resize, then multi-monitor. **Accepted.** · PO SUPPORT · QA SUPPORT (it becomes the CI compositor) · SA SUPPORT
(memory-safe, no Xvfb in the trust boundary) · UX SUPPORT · PA CONCERN: the session indicator also appears
inside the headless session when another viewer joins a shared session. **Accepted.** · EH CONCERN: shared-session
hijack, meaning a new client could take over input. Shared input needs an explicit policy. **Accepted.** · BA SUPPORT.
**Outcome:** ACCEPTED-WITH-CONDITIONS · Roadmap M7

---

## M8: Performance & encodings
**Round 1:** All SUPPORT except QA CONCERN (benchmarks and baselines published per release, accepted) and
EH CONCERN (zlib-bomb and memory limits on Tight/ZRLE inflate, accepted).
**Outcome:** ACCEPTED-WITH-CONDITIONS · Roadmap M8

---

## M9: Compatibility & polish
**Round 1:** PO SUPPORT · UX SUPPORT · SA CONCERN: installing `vncserver`/`vncviewer` names may shadow a real
TigerVNC install and confuse users or scripts, so make them opt-in symlinks. **Accepted.** · others SUPPORT.
**Outcome:** ACCEPTED-WITH-CONDITIONS · Roadmap M9

---

## M10: GPU & H.264
**Round 1:** PO SUPPORT · CA SUPPORT (encoder behind a codec port) · SA CONCERN: GPU drivers and hardware codecs are
a large C attack surface, so they stay optional and off by default. **Accepted.** · PA SUPPORT · QA CONCERN: needs
self-hosted GPU runners or manual Q3. **Accepted.** · EH SUPPORT · **BA CONCERN:** H.264 patent licensing is
unresolved. A licensing review before enabling it in release builds becomes a blocking task. **Accepted.**
**Outcome:** ACCEPTED-WITH-CONDITIONS · Roadmap M10

---

## Horizon items
- **Web interface over Tailscale (PRD-9):** DEFERRED. A new attack surface (web auth, SSRF, CSRF) needs
  its own council session with the threat model. CA confirms the sans-I/O core keeps it possible.
- **Tight security type (SEC-4):** DEFERRED. Low demand, and modern TigerVNC doesn't need it.
- **Privilege-separated agent:** DEFERRED, with SA support. Revisit after M4.
- **Greeter access:** DEFERRED. It requires root, which conflicts with ADR-0009 and would need its own ADR.

---

## Escalations to the user
- **E1. Product name "Peepin Tom".** This came up throughout the session rather than from any single item. PA
  and PO note that "peeping Tom" means a voyeur. On a remote-viewing tool, that harms trust from the people being
  viewed (PbD principle 7), distro and enterprise adoption, and searchability. CA, SE and QA have no stake.
  The binary name `ptom` is neutral. *Options:* keep it · rename the display name and keep `ptom` · rename both.
- **E2. Anonymous-TLS VeNCrypt.** See M3 above. *Council leaning:* exclude it.
