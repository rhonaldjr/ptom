# ADR-0014: Agent credential storage via farsight-passwd (no legacy passwd file)

- **Status:** Accepted · **Date:** 2026-10-23 · **Deciders:** user ("use a new approach, more secure")
- **Requirements:** SEC-7, UTL-1, PRD-8

## Context
`vncpasswd` writes `~/.vnc/passwd`: an 8-character password "encrypted" with a fixed, public DES key, which
amounts to plaintext. The user rejected writing that format.

## Decision
- `farsight-passwd` manages the **agent's** credentials in `$XDG_CONFIG_HOME/farsight/agent-credentials.toml`
  (mode 0600, atomic writes):
  - **User accounts** for authenticated, encrypted security types (VeNCrypt X509Plain, RA2/RA2_256): a username
    plus an **Argon2id** PHC hash. The server verifies the password inside the encrypted channel and never stores it.
  - **Legacy VncAuth password** (only for interop, and only if the user opts in with
    `farsight-passwd vncauth set`). VncAuth needs the server to hold the key, so it can't be hashed. It is
    sealed with XChaCha20-Poly1305 under a key held in the OS keyring (or systemd credentials for
    headless agents). It is never written in DES-obfuscated form.
- `farsight-passwd import ~/.vnc/passwd` reads a legacy file (and so does the viewer's `-passwd` option for
  compatibility), stores it the new way, and offers to delete the old file.
- Commands: `user add|remove|list`, `vncauth set|clear`, `import`. Passwords are read from a TTY without echo
  (or from stdin with `--stdin`), and are never accepted as command-line arguments.

## Consequences
- TigerVNC servers can't use files written by `farsight-passwd`. Users of those servers keep using TigerVNC's own `vncpasswd`.
- Nothing on disk reveals a password, even when the legacy VncAuth option is enabled.
