# ADR-0008: Secret storage: OS keyring and an encrypted vault

- **Status:** Accepted (council) · **Date:** 2026-10-23
- **Requirements:** PRD-10, VWR-8, VWR-9, SEC-7, UTL-1

## Decision
- `farsight-vault` stores all per-machine client data (hosts, usernames, passwords, pinned
  fingerprints, per-session settings and history) in `$XDG_DATA_HOME/farsight/vault.bin`. The file is
  encrypted with **XChaCha20-Poly1305** using a random 256-bit vault key, with an authenticated header and version.
- The vault key is stored in the OS keyring (Secret Service on Linux, Keychain on macOS). If no
  keyring is available, the key is derived from a user passphrase with **Argon2id**.
- In memory, secrets use `secrecy`/`zeroize`. Writes are atomic (write to a temp file, fsync, rename) with `0600` permissions.
- Legacy `~/.vnc/passwd` (DES-obfuscated) is *read* for compatibility and offered for import, but **never written** (user decision; see [ADR-0014](0014-agent-credentials.md)).

## Consequences
Nothing secret is stored in plaintext config. Losing the keyring entry or passphrase means losing
the saved credentials (by design). An export/backup format is a later roadmap item.
