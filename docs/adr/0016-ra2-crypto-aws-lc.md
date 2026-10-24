# ADR-0016: RSA operations for RSA-AES use aws-lc-rs, not the `rsa` crate

- **Status:** Accepted · **Date:** 2026-10-23 · **Deciders:** Security Architect (M3 implementation)
- **Requirements:** SEC-6, SEC-8 · Extends [ADR-0007](0007-crypto-stack.md)

## Context
RSA-AES (RA2, RA2_256) needs RSA PKCS#1 v1.5 encryption and decryption. The pure-Rust `rsa` crate has an unpatched
timing side-channel in decryption: the Marvin attack, RUSTSEC-2023-0071, still affected in 0.9.10 and 0.10-rc as of
2026-09. That is exactly the operation an agent performs with its long-term key for every connecting client.

## Decision
- Use **aws-lc-rs** (`Pkcs1PublicEncryptingKey`, `Pkcs1PrivateDecryptingKey`, key generation and system randomness).
  aws-lc-rs is already our rustls crypto provider (ADR-0007), so this adds no new trust root.
- A small, fuzzed DER module converts between RA2's raw (modulus, exponent) wire form and X.509 `SubjectPublicKeyInfo`.
- Peer keys must be 2048–8192 bits. The viewer generates a fresh 2048-bit key for every connection.
- AES-EAX framing uses RustCrypto `eax` + `aes`, and hashing uses `sha1`/`sha2`.
- Random numbers are 16 bytes for RA2 and 32 for RA2_256. The spec text says 16 throughout, but deployed servers
  (TigerVNC, observed as a black box) require 32 for RA2_256.
- The "ne" variants (no encryption after authentication) are not offered or accepted.

## Consequences
- RA2 interoperates with TigerVNC 1.14+ (verified in CI against Debian trixie).
- aws-lc-sys carries the OpenSSL licence for parts of its C code. It is a permissive licence and was already present
  through rustls.
