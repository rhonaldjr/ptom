# ADR-0007: Crypto stack: rustls and RustCrypto, with no anonymous TLS

- **Status:** Accepted (council) · **Date:** 2026-10-23 · **Council:** [initial roadmap](../council/2026-10-23-initial-roadmap.md)
- **Requirements:** SEC-1…SEC-9

## Decision
- TLS: **rustls** (aws-lc-rs or ring provider). Certificates are handled with `rustls-pki-types`/`webpki`, and `rcgen` generates self-signed agent certificates.
- Primitives come from RustCrypto: `des` (VncAuth legacy only), `rsa`, `aes`/`aes-gcm`, `sha1`/`sha2`, `chacha20poly1305`, `argon2`.
- **VeNCrypt X509\*** and **RA2/RA2_256** provide encrypted, server-authenticated sessions.
- **Anonymous-TLS VeNCrypt types (TLSNone/TLSVnc/TLSPlain) are not supported.** rustls has no
  anonymous DH, and those types cannot authenticate the server. Confirmed by the user (E2).
- No OpenSSL or GnuTLS dependency. `cargo-deny` bans `openssl-sys`.

## Consequences
TigerVNC clients configured to use *only* TLSVnc/TLSNone will not connect until they enable
X509Vnc or RA2. This is documented in the migration guide.
