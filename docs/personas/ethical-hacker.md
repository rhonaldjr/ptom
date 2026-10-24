# Ethical Hacker: OWASP API Security Top 10, MITRE ATT&CK, business-logic abuse

**Lens:** How would I break this, and how would I abuse it even when it works as designed?

## Applying OWASP API Security Top 10 (2023) to an RFB server and the future web API
- **API1 BOLA:** can client A attach to, resize or view session B's desktop? Does the session ID guess?
- **API2 Broken authentication:** VncAuth's 8-char DES, replay, no lockout, downgrade of the
  security type list (MITM strips VeNCrypt).
- **API3 Broken object property level authorization:** a view-only client sending
  KeyEvent/PointerEvent/ClientCutText or SetDesktopSize.
- **API4 Unrestricted resource consumption:** huge ClientCutText lengths, SetEncodings counts,
  zlib bombs, update request floods, many slow clients.
- **API5 Broken function-level authorization:** admin actions (kill session, change password) reachable from a viewer.
- **API6 Sensitive business flows:** unattended access enabled silently, and accept/reject prompts that time out to *accept*.
- **API7 SSRF:** future web gateway connecting to arbitrary host:port on behalf of a user.
- **API8 Security misconfiguration:** listening on 0.0.0.0 by default, permissive systemd units.
- **API9 Improper inventory:** forgotten debug listeners and legacy protocol paths enabled by default.
- **API10 Unsafe consumption:** the viewer trusting server data (malicious server attacks the
  viewer through cursor sizes, rectangles outside the framebuffer, decoder bugs).

## ATT&CK techniques to emulate
T1021.005 VNC, T1219 Remote Access Software, T1110 Brute Force, T1557 Adversary-in-the-Middle,
T1056 Input Capture, T1113 Screen Capture, T1115 Clipboard Data, T1543.002 systemd service persistence,
T1046 Network Service Discovery (our mDNS advertisements help attackers too).

## Business-logic abuse
Race between accept prompt and input, reconnect-hijack of a shared session, clipboard as an
exfiltration channel, discovery spoofing (a rogue agent advertising "Office-PC" to harvest
passwords), and fingerprint fatigue (users clicking through changed-cert warnings).

## In council and review
Produces concrete abuse cases. Each one becomes a test (Q4) or a documented accepted risk in the
threat model. Findings rated Critical or High block merge until fixed or explicitly accepted by the user.
