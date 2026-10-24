//! Encrypted storage for per-machine client data: hosts, credentials, pinned
//! fingerprints and connection history (ADR-0008), plus the agent's
//! credential file (ADR-0014).
//!
//! - [`Vault`] is the viewer-side store in `$XDG_DATA_HOME/farsight/vault.bin`,
//!   sealed with XChaCha20-Poly1305.
//! - [`KeySource`] supplies the 256-bit key: the OS keyring
//!   ([`KeyringKeySource`]), a passphrase stretched with Argon2id
//!   ([`PassphraseKeySource`]), or a caller-held key ([`MemoryKeySource`]).
//! - [`agent::AgentCredentials`] manages the agent's user accounts (Argon2id
//!   PHC hashes) and the optional, sealed legacy VncAuth password.
//! - [`legacy`] reads (never writes) a legacy `~/.vnc/passwd` file.
//!
//! Nothing secret is written in plaintext, logged, or shown by `Debug`.
//!
//! Layer: **Adapters** (ADR-0002).

pub mod agent;
mod crypto;
mod error;
mod format;
mod fsutil;
mod hex;
mod key;
mod keyring;
pub mod legacy;
mod model;
pub mod paths;
mod vault;

pub use error::{KeyError, VaultError};
pub use key::{
    Argon2Params, KeyMetadata, KeySource, MemoryKeySource, PassphraseKeySource, VaultKey,
};
pub use keyring::KeyringKeySource;
pub use model::{
    HistoryEntry, IdentityKind, ParseServerIdError, PinnedIdentity, SavedServer, ServerId,
    Timestamp, TrustedNetwork,
};
pub use vault::{HISTORY_LIMIT, MAX_VAULT_FILE_LEN, Vault};
