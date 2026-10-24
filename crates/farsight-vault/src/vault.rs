//! The viewer's encrypted store (ADR-0008).

use std::cmp::Reverse;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use secrecy::SecretString;
use zeroize::Zeroizing;

use crate::crypto::{self, DerivedKeys, OpenError, TAG_LEN};
use crate::error::VaultError;
use crate::format::{self, Header};
use crate::fsutil::{self, FsError, ReadError};
use crate::key::{KeyMetadata, KeySource, VaultKey};
use crate::model::{Contents, HistoryEntry, PinnedIdentity, SavedServer, ServerId, Timestamp};
use crate::paths;

/// The most history entries kept; the oldest are dropped first.
pub const HISTORY_LIMIT: usize = 1000;
/// The largest vault file accepted: 64 MiB.
pub const MAX_VAULT_FILE_LEN: u64 = 64 * 1024 * 1024;

/// HKDF domain for vault keys.
const DOMAIN: &str = "farsight-vault/v1";

/// An unlocked vault. Changes stay in memory until [`Vault::save`].
pub struct Vault {
    path: PathBuf,
    key: VaultKey,
    metadata: KeyMetadata,
    contents: Contents,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vault")
            .field("path", &self.path)
            .field("key_source", &self.metadata.kind_name())
            .field("servers", &self.contents.servers.len())
            .field("history", &self.contents.history.len())
            .finish_non_exhaustive()
    }
}

impl Vault {
    /// `$XDG_DATA_HOME/farsight/vault.bin`.
    ///
    /// # Errors
    /// [`VaultError::NoDataDir`] if neither `XDG_DATA_HOME` nor `HOME` is set.
    pub fn default_path() -> Result<PathBuf, VaultError> {
        paths::vault_file().ok_or(VaultError::NoDataDir)
    }

    /// Creates an empty vault at `path` and writes it.
    ///
    /// # Errors
    /// [`VaultError::AlreadyExists`] if something is already at `path`; key
    /// and I/O errors otherwise.
    pub fn create(path: impl Into<PathBuf>, keys: &dyn KeySource) -> Result<Self, VaultError> {
        let path = path.into();
        if fsutil::exists(&path) {
            return Err(VaultError::AlreadyExists(path));
        }
        let (key, metadata) = keys.create_key()?;
        let vault = Self {
            path,
            key,
            metadata,
            contents: Contents::default(),
        };
        vault.save()?;
        Ok(vault)
    }

    /// Opens and decrypts the vault at `path`.
    ///
    /// # Errors
    /// I/O errors, [`VaultError::TooLarge`], and everything
    /// [`Vault::from_bytes`] returns.
    pub fn open(path: impl Into<PathBuf>, keys: &dyn KeySource) -> Result<Self, VaultError> {
        let path = path.into();
        let bytes = fsutil::read_bounded(&path, MAX_VAULT_FILE_LEN).map_err(|e| match e {
            ReadError::Io(e) => io_error(e),
            ReadError::TooLarge(len) => VaultError::TooLarge(len),
        })?;
        Self::from_bytes(path, &bytes, keys)
    }

    /// Opens the vault at `path`, creating an empty one if there is none.
    ///
    /// # Errors
    /// As for [`Vault::open`] and [`Vault::create`].
    pub fn open_or_create(
        path: impl Into<PathBuf>,
        keys: &dyn KeySource,
    ) -> Result<Self, VaultError> {
        let path = path.into();
        if fsutil::exists(&path) {
            Self::open(path, keys)
        } else {
            Self::create(path, keys)
        }
    }

    /// Decrypts a vault from bytes; `path` is where [`Vault::save`] will
    /// write. Never panics, whatever the input.
    ///
    /// # Errors
    /// - [`VaultError::NotAVault`], [`VaultError::Truncated`],
    ///   [`VaultError::UnsupportedVersion`], [`VaultError::MalformedHeader`]
    ///   for structural problems;
    /// - [`VaultError::Key`] if `keys` cannot supply a key for this vault;
    /// - [`VaultError::WrongKey`] if the key is not the vault's key;
    /// - [`VaultError::Tampered`] if the key is right but authentication fails;
    /// - [`VaultError::Corrupt`] if the authenticated contents cannot be decoded.
    pub fn from_bytes(
        path: impl Into<PathBuf>,
        bytes: &[u8],
        keys: &dyn KeySource,
    ) -> Result<Self, VaultError> {
        let parsed = format::parse(bytes)?;
        let key = keys.recover_key(&parsed.header.metadata)?;
        let plaintext = DerivedKeys::derive(&key, DOMAIN)
            .open(
                &parsed.header.check,
                &parsed.header.nonce,
                parsed.aad,
                parsed.ciphertext,
            )
            .map_err(|e| match e {
                OpenError::WrongKey => VaultError::WrongKey,
                OpenError::Tampered => VaultError::Tampered,
            })?;
        let contents: Contents =
            serde_json::from_slice(&plaintext).map_err(|_| VaultError::Corrupt)?;
        Ok(Self {
            path: path.into(),
            key,
            metadata: parsed.header.metadata,
            contents,
        })
    }

    /// Encrypts the vault under a fresh random nonce.
    ///
    /// # Errors
    /// [`VaultError::Key`] if no random nonce can be obtained, or
    /// [`VaultError::TooLarge`] if the contents are too large to encrypt.
    pub fn to_bytes(&self) -> Result<Vec<u8>, VaultError> {
        let plaintext = serialize_zeroizing(&self.contents)?;
        let ciphertext_len = plaintext.len() + TAG_LEN;
        let keys = DerivedKeys::derive(&self.key, DOMAIN);
        let header = Header {
            metadata: self.metadata.clone(),
            check: *keys.check(),
            nonce: crypto::random_nonce()?,
        };
        let too_large = || VaultError::TooLarge(u64::try_from(ciphertext_len).unwrap_or(u64::MAX));
        let mut file = format::encode_header(
            &header,
            u64::try_from(ciphertext_len).map_err(|_| too_large())?,
        );
        let ciphertext = keys
            .seal(&header.nonce, &file, &plaintext)
            .ok_or_else(too_large)?;
        file.extend_from_slice(&ciphertext);
        Ok(file)
    }

    /// Writes the vault to its path atomically, with mode 0600.
    ///
    /// # Errors
    /// As for [`Vault::to_bytes`], plus I/O errors.
    pub fn save(&self) -> Result<(), VaultError> {
        let bytes = self.to_bytes()?;
        fsutil::write_atomic(&self.path, &bytes).map_err(io_error)
    }

    /// Where the vault is saved.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How the vault's key is obtained.
    #[must_use]
    pub fn key_metadata(&self) -> &KeyMetadata {
        &self.metadata
    }

    /// All saved servers, in insertion order.
    #[must_use]
    pub fn servers(&self) -> &[SavedServer] {
        &self.contents.servers
    }

    /// The saved server with this id.
    #[must_use]
    pub fn server(&self, id: &ServerId) -> Option<&SavedServer> {
        self.contents.servers.iter().find(|s| s.id == *id)
    }

    /// Inserts `server`, or replaces the saved server with the same id.
    /// Returns the replaced entry, if any.
    pub fn upsert_server(&mut self, server: SavedServer) -> Option<SavedServer> {
        if let Ok(existing) = self.server_mut(&server.id) {
            Some(std::mem::replace(existing, server))
        } else {
            self.contents.servers.push(server);
            None
        }
    }

    /// Removes a saved server. Its history entries stay, unlinked from it.
    pub fn remove_server(&mut self, id: &ServerId) -> Option<SavedServer> {
        let index = self.contents.servers.iter().position(|s| s.id == *id)?;
        for entry in &mut self.contents.history {
            if entry.server_id == Some(*id) {
                entry.server_id = None;
            }
        }
        Some(self.contents.servers.remove(index))
    }

    /// Saves a password for a server.
    ///
    /// # Errors
    /// [`VaultError::UnknownServer`] if there is no server with this id.
    pub fn set_password(
        &mut self,
        id: &ServerId,
        password: SecretString,
    ) -> Result<(), VaultError> {
        self.server_mut(id)?.set_password(password);
        Ok(())
    }

    /// Forgets a server's password.
    ///
    /// # Errors
    /// [`VaultError::UnknownServer`] if there is no server with this id.
    pub fn clear_password(&mut self, id: &ServerId) -> Result<(), VaultError> {
        self.server_mut(id)?.clear_password();
        Ok(())
    }

    /// Trusts this server identity from now on.
    ///
    /// # Errors
    /// [`VaultError::UnknownServer`] if there is no server with this id.
    pub fn pin_identity(
        &mut self,
        id: &ServerId,
        identity: PinnedIdentity,
    ) -> Result<(), VaultError> {
        self.server_mut(id)?.pinned_identity = Some(identity);
        Ok(())
    }

    /// Forgets the pinned identity.
    ///
    /// # Errors
    /// [`VaultError::UnknownServer`] if there is no server with this id.
    pub fn unpin_identity(&mut self, id: &ServerId) -> Result<(), VaultError> {
        self.server_mut(id)?.pinned_identity = None;
        Ok(())
    }

    /// Records a connection to a saved server: updates its
    /// `last_connected_at` and adds a history entry.
    ///
    /// # Errors
    /// [`VaultError::UnknownServer`] if there is no server with this id.
    pub fn record_connection(&mut self, id: &ServerId, at: Timestamp) -> Result<(), VaultError> {
        let server = self.server_mut(id)?;
        server.last_connected_at = Some(at);
        let entry = HistoryEntry {
            server_id: Some(*id),
            label: server.label.clone(),
            host: server.host.clone(),
            port: server.port,
            connected_at: at,
        };
        self.push_history(entry);
        Ok(())
    }

    /// Records a connection to a host that is not saved.
    pub fn record_unsaved_connection(&mut self, host: impl Into<String>, port: u16, at: Timestamp) {
        self.push_history(HistoryEntry {
            server_id: None,
            label: String::new(),
            host: host.into(),
            port,
            connected_at: at,
        });
    }

    /// The connection history, oldest first.
    #[must_use]
    pub fn history(&self) -> &[HistoryEntry] {
        &self.contents.history
    }

    /// Forgets all connection history (saved servers keep their
    /// `last_connected_at`).
    pub fn clear_history(&mut self) {
        self.contents.history.clear();
    }

    /// Saved servers whose label or host contains `query`, ignoring case,
    /// most recently used first. An empty query matches everything.
    #[must_use]
    pub fn search_servers(&self, query: &str) -> Vec<&SavedServer> {
        let needle = query.to_lowercase();
        let mut found: Vec<_> = self
            .contents
            .servers
            .iter()
            .rev()
            .filter(|s| s.matches(&needle))
            .collect();
        found.sort_by_key(|s| Reverse(s.recency()));
        found
    }

    /// History entries whose label or host contains `query`, ignoring
    /// case, most recent first. An empty query matches everything.
    #[must_use]
    pub fn search_history(&self, query: &str) -> Vec<&HistoryEntry> {
        let needle = query.to_lowercase();
        let mut found: Vec<_> = self
            .contents
            .history
            .iter()
            .rev()
            .filter(|e| e.matches(&needle))
            .collect();
        found.sort_by_key(|e| Reverse(e.connected_at));
        found
    }

    fn server_mut(&mut self, id: &ServerId) -> Result<&mut SavedServer, VaultError> {
        self.contents
            .servers
            .iter_mut()
            .find(|s| s.id == *id)
            .ok_or(VaultError::UnknownServer(*id))
    }

    fn push_history(&mut self, entry: HistoryEntry) {
        let history = &mut self.contents.history;
        history.push(entry);
        if history.len() > HISTORY_LIMIT {
            let excess = history.len() - HISTORY_LIMIT;
            history.drain(..excess);
        }
    }
}

fn io_error(e: FsError) -> VaultError {
    VaultError::Io {
        op: e.op,
        path: e.path,
        source: e.source,
    }
}

/// Serializes into a buffer sized up front, so no reallocation leaves
/// copies of secrets behind in freed memory.
fn serialize_zeroizing<T: serde::Serialize>(value: &T) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, value).map_err(|_| VaultError::Corrupt)?;
    let mut buffer = Zeroizing::new(Vec::with_capacity(counter.0));
    serde_json::to_writer(&mut *buffer, value).map_err(|_| VaultError::Corrupt)?;
    Ok(buffer)
}

struct ByteCounter(usize);

impl io::Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
