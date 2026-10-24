# ADR-0017: Server identity verification and pinning (TOFU)

- **Status:** Accepted (council conditions from M3) · **Date:** 2026-10-23
- **Requirements:** SEC-1, SEC-9, VWR-9, VWR-14

## Decision
- **Fingerprint:** SHA-256 of the TLS leaf certificate (VeNCrypt X509*), or of the RA2 public key exactly as sent on
  the wire. This is what users see, as two lines of colon-separated hex, and what the vault pins.
- **TLS:** the handshake verifies signatures (the server must hold the certificate's key) but defers *trust*.
  Afterwards the session continues without asking only if the certificate chains to an OS-trusted CA for the host
  name, or its fingerprint matches the pinned one. Otherwise it pauses with `IdentityConfirmationRequired`
  **before any credentials are sent**.
- **RA2:** the authenticator pauses after receiving the server key and before sending the viewer's key. A pinned
  match continues automatically.
- **First contact:** the user sees "Is this the right computer?" with the fingerprint, and accepting pins it.
- **Changed identity:** the warning says "This computer's identity has changed". *Disconnect* is the default button,
  and *Trust the new identity* only becomes active after the user ticks "I have checked the new fingerprint…"
  (protection against fingerprint fatigue).
- **Downgrade protection:** a server with a pinned identity must offer an encrypted security type. Otherwise the
  connection is refused with an explanation (a man-in-the-middle may have stripped encryption from the offer).
- Successful connections are saved and their identity pinned automatically (P-7). Passwords are saved only when the
  user ticks "Remember on this computer".
