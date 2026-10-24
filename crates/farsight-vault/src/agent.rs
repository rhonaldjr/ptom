//! The agent's credentials (ADR-0014), kept in
//! `$XDG_CONFIG_HOME/farsight/agent-credentials.toml` (mode 0600, atomic
//! writes):
//!
//! - **user accounts**, each a username and an Argon2id PHC hash, used by
//!   the encrypted security types (VeNCrypt `X509Plain`, RA2);
//! - an optional **legacy VncAuth password**, which VncAuth needs in
//!   recoverable form. It is sealed with XChaCha20-Poly1305 under a key from
//!   a [`KeySource`] (the OS keyring by default) and never written in the
//!   DES-obfuscated `~/.vnc/passwd` format.
//!
//! ```toml
//! version = 1
//!
//! [vncauth]
//! key_check = "<hex>"
//! nonce = "<hex>"
//! ciphertext = "<hex>"
//!
//! [vncauth.key]
//! source = "keyring"
//!
//! [[user]]
//! name = "alice"
//! hash = "$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>"
//! ```

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::crypto::{self, CHECK_LEN, DerivedKeys, NONCE_LEN, OpenError};
use crate::error::KeyError;
use crate::fsutil::{self, FsError, ReadError};
use crate::hex;
use crate::key::{Argon2Params, KeyMetadata, KeySource};
use crate::paths;

/// The only file version so far.
const FILE_VERSION: u32 = 1;
/// The largest credentials file accepted: 1 MiB.
const MAX_FILE_LEN: u64 = 1024 * 1024;
/// HKDF domain and AEAD associated data for the sealed VncAuth password.
const VNCAUTH_DOMAIN: &str = "farsight-agent/v1/vncauth";

/// Longest username accepted, in bytes (RA2 sends it with a one-byte length).
pub const MAX_USERNAME_LEN: usize = 255;
/// Longest password accepted, in bytes.
pub const MAX_PASSWORD_LEN: usize = 1024;

/// Errors from loading, changing or saving agent credentials.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    /// Neither `XDG_CONFIG_HOME` nor `HOME` is set.
    #[error("could not find a config directory: set XDG_CONFIG_HOME or HOME")]
    NoConfigDir,
    /// The file is larger than any credentials file Farsight writes.
    #[error("the credentials file is too large ({0} bytes)")]
    TooLarge(u64),
    /// The file is not valid TOML of the expected shape.
    #[error("the credentials file could not be read: {0}")]
    Parse(String),
    /// The file was written by a newer, incompatible version.
    #[error("the credentials file uses version {0}, which this version of Farsight cannot read")]
    UnsupportedVersion(u32),
    /// The same username appears twice.
    #[error("the credentials file lists user {0:?} more than once")]
    DuplicateUser(String),
    /// A username is not acceptable.
    #[error("invalid username: {0}")]
    InvalidUsername(&'static str),
    /// A password is not acceptable.
    #[error("invalid password: {0}")]
    InvalidPassword(&'static str),
    /// Argon2 hashing failed.
    #[error("password hashing failed")]
    Hash,
    /// Obtaining the sealing key failed.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// The key does not match the one the VncAuth password was sealed with.
    #[error("the key does not unlock the stored VNC Authentication password")]
    WrongKey,
    /// The sealed VncAuth password fails authentication.
    #[error("the stored VNC Authentication password has been modified or damaged")]
    Tampered,
    /// A filesystem operation failed.
    #[error("could not {op} {}: {source}", path.display())]
    Io {
        /// What was being attempted.
        op: &'static str,
        /// The file involved.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

impl From<FsError> for AgentError {
    fn from(e: FsError) -> Self {
        Self::Io {
            op: e.op,
            path: e.path,
            source: e.source,
        }
    }
}

/// Whether [`AgentCredentials::set_user`] added a user or changed one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserChange {
    /// A new user.
    Added,
    /// An existing user's password was replaced.
    PasswordChanged,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct UserRecord {
    name: String,
    hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SealedSecret {
    #[serde(with = "hex::array")]
    key_check: [u8; CHECK_LEN],
    #[serde(with = "hex::array")]
    nonce: [u8; NONCE_LEN],
    #[serde(with = "hex::vec")]
    ciphertext: Vec<u8>,
    key: KeyMetadata,
}

#[derive(Serialize, Deserialize)]
struct FileRepr {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vncauth: Option<SealedSecret>,
    #[serde(default, rename = "user", skip_serializing_if = "Vec::is_empty")]
    users: Vec<UserRecord>,
}

/// The agent's credential file, loaded into memory. Changes stay in memory
/// until [`AgentCredentials::save`].
pub struct AgentCredentials {
    path: PathBuf,
    users: Vec<UserRecord>,
    vncauth: Option<SealedSecret>,
    params: Argon2Params,
    dummy_hash: OnceLock<Option<String>>,
}

impl fmt::Debug for AgentCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentCredentials")
            .field("path", &self.path)
            .field("users", &self.users.len())
            .field("vncauth", &self.vncauth.is_some())
            .finish_non_exhaustive()
    }
}

impl AgentCredentials {
    /// `$XDG_CONFIG_HOME/farsight/agent-credentials.toml`.
    ///
    /// # Errors
    /// [`AgentError::NoConfigDir`] if neither `XDG_CONFIG_HOME` nor `HOME` is set.
    pub fn default_path() -> Result<PathBuf, AgentError> {
        paths::agent_credentials_file().ok_or(AgentError::NoConfigDir)
    }

    /// Loads the file at `path` (an empty set if it does not exist), hashing
    /// new passwords with [`Argon2Params::LOGIN_DEFAULT`].
    ///
    /// # Errors
    /// I/O, size, parse and validation errors.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, AgentError> {
        Self::load_with_params(path, Argon2Params::LOGIN_DEFAULT)
    }

    /// Like [`AgentCredentials::load`], hashing new passwords with `params`.
    /// Existing hashes keep the parameters they were made with.
    ///
    /// # Errors
    /// I/O, size, parse and validation errors.
    pub fn load_with_params(
        path: impl Into<PathBuf>,
        params: Argon2Params,
    ) -> Result<Self, AgentError> {
        let path = path.into();
        let repr = if fsutil::exists(&path) {
            let bytes = fsutil::read_bounded(&path, MAX_FILE_LEN).map_err(|e| match e {
                ReadError::Io(e) => AgentError::from(e),
                ReadError::TooLarge(len) => AgentError::TooLarge(len),
            })?;
            parse(&bytes)?
        } else {
            FileRepr {
                version: FILE_VERSION,
                vncauth: None,
                users: Vec::new(),
            }
        };
        Ok(Self {
            path,
            users: repr.users,
            vncauth: repr.vncauth,
            params,
            dummy_hash: OnceLock::new(),
        })
    }

    /// Writes the file atomically, with mode 0600.
    ///
    /// # Errors
    /// Serialization or I/O errors.
    pub fn save(&self) -> Result<(), AgentError> {
        let repr = FileRepr {
            version: FILE_VERSION,
            vncauth: self.vncauth.clone(),
            users: self.users.clone(),
        };
        let body = toml::to_string(&repr).map_err(|e| AgentError::Parse(e.to_string()))?;
        let text = format!(
            "# Farsight agent credentials. Managed by farsight-passwd; do not edit by hand.\n\
             # Holds password hashes and a sealed password, never plaintext passwords.\n\n{body}"
        );
        fsutil::write_atomic(&self.path, text.as_bytes())?;
        Ok(())
    }

    /// Where the file is saved.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Usernames, in the order they were added.
    pub fn usernames(&self) -> impl Iterator<Item = &str> {
        self.users.iter().map(|u| u.name.as_str())
    }

    /// Whether `name` is a user.
    #[must_use]
    pub fn has_user(&self, name: &str) -> bool {
        self.users.iter().any(|u| u.name == name)
    }

    /// Adds a user, or replaces an existing user's password.
    ///
    /// # Errors
    /// [`AgentError::InvalidUsername`], [`AgentError::InvalidPassword`] or
    /// [`AgentError::Hash`].
    pub fn set_user(
        &mut self,
        name: &str,
        password: &SecretString,
    ) -> Result<UserChange, AgentError> {
        validate_username(name)?;
        let password = password.expose_secret().as_bytes();
        if password.is_empty() {
            return Err(AgentError::InvalidPassword("it is empty"));
        }
        if password.len() > MAX_PASSWORD_LEN {
            return Err(AgentError::InvalidPassword("it is longer than 1024 bytes"));
        }
        let hash = self.hash(password)?;
        if let Some(user) = self.users.iter_mut().find(|u| u.name == name) {
            user.hash = hash;
            Ok(UserChange::PasswordChanged)
        } else {
            self.users.push(UserRecord {
                name: name.to_owned(),
                hash,
            });
            Ok(UserChange::Added)
        }
    }

    /// Removes a user. Returns whether there was one.
    pub fn remove_user(&mut self, name: &str) -> bool {
        let before = self.users.len();
        self.users.retain(|u| u.name != name);
        self.users.len() != before
    }

    /// Checks a username and password.
    ///
    /// Takes about the same time whether the user is unknown or the password
    /// is wrong: an unknown user is checked against a dummy hash made with
    /// the same parameters, and usernames are compared in constant time.
    #[must_use]
    pub fn verify(&self, username: &str, password: &[u8]) -> bool {
        let mut stored = None;
        for user in &self.users {
            if bool::from(user.name.as_bytes().ct_eq(username.as_bytes())) {
                stored = Some(user.hash.as_str());
            }
        }
        let known = stored.is_some();
        let Some(candidate) = stored.or_else(|| self.dummy_hash()) else {
            return false;
        };
        let matched = if let Ok(hash) = PasswordHash::new(candidate) {
            Argon2::default().verify_password(password, &hash).is_ok()
        } else {
            // Malformed stored hash: spend the same time, then fail.
            if let Some(dummy) = self.dummy_hash().and_then(|d| PasswordHash::new(d).ok()) {
                let _ = Argon2::default().verify_password(password, &dummy);
            }
            false
        };
        known & matched
    }

    /// Whether a VncAuth password is stored.
    #[must_use]
    pub fn has_vncauth_password(&self) -> bool {
        self.vncauth.is_some()
    }

    /// Seals and stores the legacy VncAuth password, replacing any earlier
    /// one. VncAuth uses only the first 8 bytes.
    ///
    /// # Errors
    /// [`AgentError::InvalidPassword`] if it is empty or too long, or a key
    /// error from `keys`.
    pub fn set_vncauth_password(
        &mut self,
        password: &SecretString,
        keys: &dyn KeySource,
    ) -> Result<(), AgentError> {
        let bytes = password.expose_secret().as_bytes();
        if bytes.is_empty() {
            return Err(AgentError::InvalidPassword("it is empty"));
        }
        if bytes.len() > MAX_PASSWORD_LEN {
            return Err(AgentError::InvalidPassword("it is longer than 1024 bytes"));
        }
        let (key, metadata) = keys.create_key()?;
        let derived = DerivedKeys::derive(&key, VNCAUTH_DOMAIN);
        let nonce = crypto::random_nonce()?;
        let ciphertext = derived
            .seal(&nonce, VNCAUTH_DOMAIN.as_bytes(), bytes)
            .ok_or(AgentError::InvalidPassword("it is too long"))?;
        self.vncauth = Some(SealedSecret {
            key_check: *derived.check(),
            nonce,
            ciphertext,
            key: metadata,
        });
        Ok(())
    }

    /// Unseals the stored VncAuth password, if there is one.
    ///
    /// # Errors
    /// A key error from `keys`, [`AgentError::WrongKey`] or
    /// [`AgentError::Tampered`].
    pub fn vncauth_password(
        &self,
        keys: &dyn KeySource,
    ) -> Result<Option<SecretString>, AgentError> {
        let Some(sealed) = &self.vncauth else {
            return Ok(None);
        };
        let key = keys.recover_key(&sealed.key)?;
        let plaintext = DerivedKeys::derive(&key, VNCAUTH_DOMAIN)
            .open(
                &sealed.key_check,
                &sealed.nonce,
                VNCAUTH_DOMAIN.as_bytes(),
                &sealed.ciphertext,
            )
            .map_err(|e| match e {
                OpenError::WrongKey => AgentError::WrongKey,
                OpenError::Tampered => AgentError::Tampered,
            })?;
        let text = std::str::from_utf8(&plaintext).map_err(|_| AgentError::Tampered)?;
        Ok(Some(SecretString::from(text)))
    }

    /// Forgets the VncAuth password. Returns whether there was one.
    pub fn clear_vncauth_password(&mut self) -> bool {
        self.vncauth.take().is_some()
    }

    fn hash(&self, password: &[u8]) -> Result<String, AgentError> {
        let hasher = self.params.hasher()?;
        let hash: PasswordHash = hasher
            .hash_password(password)
            .map_err(|_| AgentError::Hash)?;
        Ok(hash.to_string())
    }

    fn dummy_hash(&self) -> Option<&str> {
        self.dummy_hash
            .get_or_init(|| {
                let mut random = Zeroizing::new([0u8; 32]);
                getrandom::fill(random.as_mut()).ok()?;
                self.hash(random.as_ref()).ok()
            })
            .as_deref()
    }
}

fn parse(bytes: &[u8]) -> Result<FileRepr, AgentError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| AgentError::Parse("the file is not UTF-8 text".into()))?;
    // Report only the message, never a snippet of the file.
    let repr: FileRepr = toml::from_str(text).map_err(|e| AgentError::Parse(e.message().into()))?;
    if repr.version != FILE_VERSION {
        return Err(AgentError::UnsupportedVersion(repr.version));
    }
    for (i, user) in repr.users.iter().enumerate() {
        validate_username(&user.name)?;
        if repr.users.iter().skip(i + 1).any(|u| u.name == user.name) {
            return Err(AgentError::DuplicateUser(user.name.clone()));
        }
    }
    Ok(repr)
}

fn validate_username(name: &str) -> Result<(), AgentError> {
    if name.is_empty() {
        return Err(AgentError::InvalidUsername("it is empty"));
    }
    if name.len() > MAX_USERNAME_LEN {
        return Err(AgentError::InvalidUsername("it is longer than 255 bytes"));
    }
    if name.chars().any(char::is_control) {
        return Err(AgentError::InvalidUsername(
            "it contains control characters",
        ));
    }
    if name.trim() != name {
        return Err(AgentError::InvalidUsername(
            "it starts or ends with whitespace",
        ));
    }
    Ok(())
}
