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
| API4 resource exhaustion (cut text, encodings count, zlib bombs) | Bounded decoders · fuzzing | M1, M8 |
| API3 view-only client sends input | Enforced by `ServerSession` (tested) | M4 ✅ |
| API10 malicious server → viewer RCE/DoS | Memory-safe decoders · fuzzing · bounds checks | M2 |
| T1046 discovery recon / rogue-agent spoofing | Minimal TXT · unverified-until-fingerprint | M6 |
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
| Supply-chain compromise | cargo-deny · provenance attestations · SHA-pinned actions | M0 |

## Accepted risks
_None yet. Waivers of Critical/High findings must be approved by the user and recorded here._
