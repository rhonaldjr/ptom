//! What the vault holds. Serialized as JSON *inside* the ciphertext.

use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use secrecy::SecretString;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::KeyError;
use crate::hex;

/// Seconds since the Unix epoch, UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(u64);

impl Timestamp {
    /// The current time (the epoch if the clock is before 1970).
    #[must_use]
    pub fn now() -> Self {
        Self(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        )
    }

    /// A time given in Unix seconds.
    #[must_use]
    pub const fn from_unix_secs(secs: u64) -> Self {
        Self(secs)
    }

    /// This time in Unix seconds.
    #[must_use]
    pub const fn as_unix_secs(self) -> u64 {
        self.0
    }
}

/// A stable, random 128-bit identifier for a saved server.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServerId([u8; 16]);

impl ServerId {
    /// A fresh random id.
    ///
    /// # Errors
    /// [`KeyError::Random`] if the OS random number generator fails.
    pub fn generate() -> Result<Self, KeyError> {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).map_err(|_| KeyError::Random)?;
        Ok(Self(id))
    }

    /// The raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// An id from raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for ServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(&self.0))
    }
}

impl fmt::Debug for ServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ServerId({self})")
    }
}

/// The text is not 32 hex digits.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a server id is 32 hexadecimal digits")]
pub struct ParseServerIdError;

impl FromStr for ServerId {
    type Err = ParseServerIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        hex::decode_array(s).map(Self).ok_or(ParseServerIdError)
    }
}

impl Serialize for ServerId {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        hex::array::serialize(&self.0, ser)
    }
}

impl<'de> Deserialize<'de> for ServerId {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        hex::array::deserialize(de).map(Self)
    }
}

/// What kind of server identity a fingerprint covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdentityKind {
    /// An X.509 certificate (VeNCrypt X509*).
    X509,
    /// An RSA public key (RA2 and `RA2_256`).
    Rsa,
}

/// A server identity the user has chosen to trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PinnedIdentity {
    /// What the fingerprint covers.
    pub kind: IdentityKind,
    /// SHA-256 of the DER certificate or the encoded public key.
    #[serde(with = "hex::array")]
    pub sha256: [u8; 32],
}

/// A saved server ("address book" entry).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedServer {
    /// Stable identifier.
    pub id: ServerId,
    /// The name the user gave it.
    pub label: String,
    /// Host name or address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Username, for security types that use one.
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default, with = "secret_option")]
    password: Option<SecretString>,
    /// The trusted server identity, if pinned.
    #[serde(default)]
    pub pinned_identity: Option<PinnedIdentity>,
    /// Connect without sending input.
    #[serde(default)]
    pub view_only: bool,
    /// When it was saved.
    pub created_at: Timestamp,
    /// When it was last connected to.
    #[serde(default)]
    pub last_connected_at: Option<Timestamp>,
}

impl SavedServer {
    /// A new entry with a fresh id and no credentials.
    ///
    /// # Errors
    /// [`KeyError::Random`] if the OS random number generator fails.
    pub fn new(
        label: impl Into<String>,
        host: impl Into<String>,
        port: u16,
        created_at: Timestamp,
    ) -> Result<Self, KeyError> {
        Ok(Self {
            id: ServerId::generate()?,
            label: label.into(),
            host: host.into(),
            port,
            username: None,
            password: None,
            pinned_identity: None,
            view_only: false,
            created_at,
            last_connected_at: None,
        })
    }

    /// The saved password, if any.
    #[must_use]
    pub fn password(&self) -> Option<&SecretString> {
        self.password.as_ref()
    }

    /// Replaces the saved password.
    pub fn set_password(&mut self, password: SecretString) {
        self.password = Some(password);
    }

    /// Forgets the saved password.
    pub fn clear_password(&mut self) {
        self.password = None;
    }

    pub(crate) fn matches(&self, needle: &str) -> bool {
        contains_folded(&self.label, needle) || contains_folded(&self.host, needle)
    }

    /// The time used to order search results: last connection, else creation.
    pub(crate) fn recency(&self) -> Timestamp {
        self.last_connected_at.unwrap_or(self.created_at)
    }
}

/// One past connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The saved server, if the connection was to one that still exists.
    #[serde(default)]
    pub server_id: Option<ServerId>,
    /// The label at the time (empty for unsaved hosts).
    #[serde(default)]
    pub label: String,
    /// Host name or address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// When the connection was made.
    pub connected_at: Timestamp,
}

impl HistoryEntry {
    pub(crate) fn matches(&self, needle: &str) -> bool {
        contains_folded(&self.label, needle) || contains_folded(&self.host, needle)
    }
}

/// The whole decrypted payload.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Contents {
    #[serde(default)]
    pub(crate) servers: Vec<SavedServer>,
    #[serde(default)]
    pub(crate) history: Vec<HistoryEntry>,
}

/// `needle` must already be lower-cased.
fn contains_folded(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(needle)
}

/// Serde for `Option<SecretString>`: exposes the secret only to the
/// serializer, which writes into a zeroized buffer.
mod secret_option {
    use secrecy::{ExposeSecret, SecretString};
    use serde::{Deserialize, Deserializer, Serializer};

    // Serde's `serialize_with` passes `&Option<_>`.
    #[allow(clippy::ref_option)]
    pub(super) fn serialize<S: Serializer>(
        value: &Option<SecretString>,
        ser: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(secret) => ser.serialize_some(secret.expose_secret()),
            None => ser.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        de: D,
    ) -> Result<Option<SecretString>, D::Error> {
        Ok(Option::<String>::deserialize(de)?.map(SecretString::from))
    }
}
