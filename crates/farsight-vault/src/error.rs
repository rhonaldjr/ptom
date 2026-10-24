//! Typed errors. Messages are written for users and never include secrets.

use std::io;
use std::path::PathBuf;

use crate::model::ServerId;

/// Something went wrong obtaining or deriving a key.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum KeyError {
    /// The OS keyring has no key for this store.
    #[error("the OS keyring has no key for this store")]
    NotFound,
    /// The OS keyring is locked, or no keyring service is running.
    #[error("the OS keyring is locked or not available: {0}")]
    Unavailable(String),
    /// The OS keyring reported some other failure.
    #[error("the OS keyring failed: {0}")]
    Keyring(String),
    /// The key stored in the OS keyring is not a 256-bit key.
    #[error("the key stored in the OS keyring is not a valid Farsight key")]
    Malformed,
    /// This platform has no supported OS keyring.
    #[error("no supported OS keyring on this platform")]
    Unsupported,
    /// Argon2 parameters are out of the accepted range.
    #[error("the key-derivation parameters are out of range")]
    BadParams,
    /// Argon2 key derivation failed.
    #[error("key derivation failed")]
    Kdf,
    /// The data was locked with a different kind of key source.
    #[error("this data is locked with {found}, not {offered}")]
    Mismatch {
        /// The kind of key source the data needs.
        found: &'static str,
        /// The kind of key source that was offered.
        offered: &'static str,
    },
    /// The operating system's random number generator failed.
    #[error("the operating system's random number generator failed")]
    Random,
}

/// Errors from opening, changing or saving a [`Vault`](crate::Vault).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VaultError {
    /// The file does not start with the vault magic bytes.
    #[error("the file is not a Farsight vault")]
    NotAVault,
    /// The vault was written by a newer, incompatible format version.
    #[error("the vault uses format version {0}, which this version of Farsight cannot read")]
    UnsupportedVersion(u16),
    /// The file ends before the header or ciphertext is complete.
    #[error("the vault file is truncated")]
    Truncated,
    /// The header is structurally invalid.
    #[error("the vault header is malformed: {0}")]
    MalformedHeader(&'static str),
    /// The key does not match the one the vault was locked with.
    #[error("the key does not unlock this vault (wrong passphrase or keyring entry)")]
    WrongKey,
    /// The key is right but the contents fail authentication.
    #[error("the vault has been modified or damaged and cannot be trusted")]
    Tampered,
    /// The contents authenticate but cannot be decoded.
    #[error("the vault contents could not be decoded")]
    Corrupt,
    /// The file is larger than any vault Farsight writes.
    #[error("the vault file is too large ({0} bytes)")]
    TooLarge(u64),
    /// No saved server has this id.
    #[error("no saved server has id {0}")]
    UnknownServer(ServerId),
    /// [`Vault::create`](crate::Vault::create) found an existing file.
    #[error("a vault already exists at {}", .0.display())]
    AlreadyExists(PathBuf),
    /// Neither `XDG_DATA_HOME` nor `HOME` is set.
    #[error("could not find a data directory: set XDG_DATA_HOME or HOME")]
    NoDataDir,
    /// Obtaining the key failed.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// A filesystem operation failed.
    #[error("could not {op} {}: {source}", path.display())]
    Io {
        /// What was being attempted, e.g. "read".
        op: &'static str,
        /// The file involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}
