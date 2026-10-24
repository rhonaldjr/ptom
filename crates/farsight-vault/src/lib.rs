//! Encrypted storage for per-machine client data: hosts, credentials, pinned
//! fingerprints, settings and history (ADR-0008). Uses XChaCha20-Poly1305 with
//! a key held in the OS keyring, or derived via Argon2id when no keyring exists.
//!
//! Layer: **Adapters** (ADR-0002). Implements the credential-store port defined
//! in `farsight-client`.
//!
//! Roadmap: M3.
