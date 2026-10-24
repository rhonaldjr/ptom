# Threat Model (living document)

Owners: Security Architect and Ethical Hacker. Update it whenever attack surface changes (see [code review](../process/code-review.md)).

## Assets
Remote desktop contents (screen, clipboard, keystrokes) · credentials and the vault · agent host
integrity · viewer host integrity · the local user's awareness that they are being viewed.

## Trust boundaries
1. Network ↔ agent RFB parser (untrusted input before auth)
2. Network ↔ viewer decoders (a malicious server attacks the viewer)
3. Agent ↔ compositor / portal / X server (capture and input injection)
4. Viewer ↔ vault / OS keyring
5. *(future)* Internet ↔ web gateway

## Threats → mitigations (initial)
| Threat (ATT&CK / OWASP) | Mitigation | Milestone |
|---|---|---|
| T1110 brute force on VncAuth | Per-address lockout with exponential backoff · VncAuth gated to loopback | M1, M4 ✅ |
| T1557 MITM / security-type downgrade | X509/RA2 with TOFU pinning; identity checked before credentials are sent; pinned servers must offer encryption (ADR-0017) | M3 ✅ |
| Marvin timing attack on RSA decryption (RA2) | aws-lc-rs constant-time PKCS#1 v1.5, not the `rsa` crate (ADR-0016) | M3 ✅ |
| Credential theft from disk | XChaCha20-Poly1305 vault with the key in the OS keyring; agent passwords stored as Argon2id hashes; `~/.vnc/passwd` never written (ADR-0008, ADR-0014) | M3 ✅ |
| Fingerprint fatigue (users click through changed-key warnings) | A changed identity needs an explicit extra acknowledgement; Disconnect is the default | M3 ✅ |
| API4 resource exhaustion (cut text, encodings count, zlib bombs) | Bounded decoders and inflate output · fence payloads ≤ 64 bytes · RRE subrect count ≤ area and checked against the bytes present · desktop names ≤ 1 KiB · fuzzing | M1, M8 ✅ |
| A viewer floods the agent's output (continuous updates) | Memory is bounded by TCP backpressure (each write is awaited); the two-update window (only replies to fences actually sent count) keeps latency low; fences only after negotiation | M8 ✅ |
| **Accepted risk:** a server renames its desktop to look like another tab | Names sanitised; empty names fall back to the address ([M8 review](../reviews/2026-10-24-m8-review.md)) | M8 |
| Malicious server stalls the viewer with overlapping RRE subrects (API4) | Total subrect area capped at twice the rectangle ([review](../reviews/2026-10-24-m8-review.md)) | M8 ✅ |
| API3 view-only client sends input | Enforced by `ServerSession` (tested) | M4 ✅ |
| API10 malicious server → viewer RCE/DoS | Memory-safe decoders · fuzzing · bounds checks | M2 |
| T1046 discovery recon / rogue-agent spoofing | Minimal TXT · unverified-until-fingerprint · agent announces only on user-trusted networks (ADR-0019) | M6 ✅ |
| T1543.002 persistence via systemd | User unit only, shipped disabled · refuses root · hardened (ADR-0009, U-6) | M4 ✅ |
| Silent surveillance of the local user | Presence tray icon plus a notification per connection; approval prompt for non-owners, denied after 30 s (U-2, U-4) | M4 ✅ |
| T1046 from our own prober (IDS alerts, AUP breach on foreign networks) | Active probing only on user-marked networks · local subnets only · rate-limited · banner only; fuzz-tested that only one version message is ever sent (ADR-0012) | M3.5 ✅ |
| Spoofed discovered server harvests VncAuth responses (T1557/T1110) | Unverified until TOFU · credentials bound to verified identity, never IP · no auto-send | M3.5, M6 |
| Malicious advertised names (bidi/control chars) in tray or list | `sanitize_name` strips control, bidi and invisible characters and caps length (fuzzed) | M3.5 ✅, M6 |
| Malicious advertised names (bidi/control chars) in tray or list | Sanitise and length-cap before rendering | M3.5, M6 |
| Desktop data leaking from the X11 backend (clipboard, keystrokes, screen) | Uses the agent user's own X connection and auth only; clipboard capped at 1 MiB both ways; clipboard text, keysyms and pixels never logged; temporary keycode remaps restored on key release and on shutdown | M5 |
| Pre-auth slot exhaustion (idle/half-open connections, T1499) | Login deadline · caps on unadmitted connections overall and per address · write timeout ([review](../reviews/2026-10-24-agent-security-review.md)) | M4 ✅ |
| Approval-prompt flooding / fatigue (T1621) | One prompt at a time · denial counts as a failed login · prompt names the security type | M4 ✅ |
| Keystrokes queued during approval, replayed on Allow (T1056) | Data before `ServerInit` closes the connection | M4 ✅ |
| STARTTLS-style plaintext injection across TLS/RA2 upgrade (CWE-74) | Upgrade refused with unread input | M3/M4 ✅ |
| Discovery spoofing using a saved computer's name | Credentials bound to address and identity · impersonation warning in list and tray | M3.5/M6 ✅ |
| Other local users reaching a headless desktop's X server (no X authority) | Agent umask 077 before Xwayland binds; socket then chmod 0700 (not via symlinks); no abstract socket; `/tmp/.X11-unix` must be root- or user-owned and sticky; Wayland socket in the user's 0700 runtime dir ([ADR-0020](../adr/0020-headless-desktops.md)) | M7 ✅ |
| Unauthenticated loopback access to a headless desktop (no prompt; T1021.005) | A headless desktop refuses to start without a password and never offers None ([review](../reviews/2026-10-24-m7-headless-review.md)) | M7 ✅ |
| Shared-session hijack: a second viewer seizes input or resizes (EH, M7 council) | One controller at a time; others' input, clipboard and resize requests refused until hand-over (idle 10 s) or the owner takes over; held keys released on hand-over (U-9, P-38) | M7 ✅ |
| Watchers harvesting copied secrets (T1115) | On headless desktops the desktop's clipboard goes only to the controller or the owner | M7 ✅ |
| **Accepted risk:** a controller keeps control by sending input every few seconds | Only the owner can take over then (P-38) | M7 |
| Silent co-viewing of a headless desktop | Red frame inside the desktop whenever 2+ viewers are connected; presence tray and notifications on the user's main desktop (P-40) | M7 ✅ |
| Resize abuse (huge or many screens → memory exhaustion) | Layout validated: ≤ 16 screens, ≤ 8192 px a side, ≤ 33.5 MP, in bounds, no overlaps; only the controller may resize; one resize per 250 ms; frames not rendered or copied while the agent is behind | M7 ✅ |
| Apps in the session exhausting the compositor (clipboard transfers that never finish) | At most 8 transfers at once; thread-spawn failure is handled, never a panic | M7 ✅ |
| Supply-chain compromise | cargo-deny · provenance attestations · SHA-pinned actions | M0 |

## Accepted risks
_None yet. Waivers of Critical/High findings must be approved by the user and recorded here._
