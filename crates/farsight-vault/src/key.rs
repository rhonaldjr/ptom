//! Keys and where they come from.

use std::fmt;

use argon2::{Algorithm, Argon2, Version};
use secrecy::{ExposeSecret, SecretBox, SecretString};
use serde::{Deserialize, Serialize};

use crate::error::KeyError;
use crate::hex;

/// Length of an Argon2id salt, in bytes.
pub const SALT_LEN: usize = 16;

/// A 256-bit symmetric key. Zeroized on drop; never shown by `Debug`.
pub struct VaultKey(SecretBox<[u8; VaultKey::LEN]>);

impl VaultKey {
    /// Key length in bytes.
    pub const LEN: usize = 32;

    /// Wraps existing key bytes. The caller should zeroize its own copy.
    #[must_use]
    pub fn from_bytes(bytes: [u8; Self::LEN]) -> Self {
        Self(SecretBox::new(Box::new(bytes)))
    }

    /// Generates a fresh random key.
    ///
    /// # Errors
    /// [`KeyError::Random`] if the OS random number generator fails.
    pub fn generate() -> Result<Self, KeyError> {
        let mut outcome = Ok(());
        let key = SecretBox::init_with_mut(|k: &mut [u8; Self::LEN]| outcome = getrandom::fill(k));
        outcome.map_err(|_| KeyError::Random)?;
        Ok(Self(key))
    }

    /// Copies a key out of a slice, which must be exactly [`Self::LEN`] bytes.
    pub(crate) fn from_slice(bytes: &[u8]) -> Result<Self, KeyError> {
        if bytes.len() != Self::LEN {
            return Err(KeyError::Malformed);
        }
        Ok(Self(SecretBox::init_with_mut(|k: &mut [u8; Self::LEN]| {
            k.copy_from_slice(bytes);
        })))
    }

    pub(crate) fn expose(&self) -> &[u8; Self::LEN] {
        self.0.expose_secret()
    }

    pub(crate) fn duplicate(&self) -> Self {
        // `from_slice` cannot fail here: the length always matches.
        Self(SecretBox::init_with_mut(|k: &mut [u8; Self::LEN]| {
            k.copy_from_slice(self.expose());
        }))
    }
}

impl fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VaultKey([REDACTED])")
    }
}

/// Argon2id cost parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Argon2Params {
    m_cost_kib: u32,
    t_cost: u32,
    p_cost: u32,
}

impl Argon2Params {
    /// For unlocking a vault with a passphrase: 64 MiB, 3 passes, 1 lane
    /// (RFC 9106 §4, second recommended option, single-lane).
    pub const VAULT_DEFAULT: Self = Self {
        m_cost_kib: 64 * 1024,
        t_cost: 3,
        p_cost: 1,
    };

    /// For hashing login passwords on the agent: 19 MiB, 2 passes, 1 lane
    /// (the OWASP Password Storage Cheat Sheet minimum for Argon2id).
    pub const LOGIN_DEFAULT: Self = Self {
        m_cost_kib: 19 * 1024,
        t_cost: 2,
        p_cost: 1,
    };

    /// Largest memory cost accepted from a file: 1 GiB.
    pub const MAX_M_COST_KIB: u32 = 1024 * 1024;
    /// Largest number of passes accepted from a file.
    pub const MAX_T_COST: u32 = 64;
    /// Largest number of lanes accepted from a file.
    pub const MAX_P_COST: u32 = 16;

    /// Checks and builds a parameter set. Low values are allowed so tests
    /// run fast; production callers should use the defaults.
    ///
    /// # Errors
    /// [`KeyError::BadParams`] if a value is outside what Argon2 accepts or
    /// above the `MAX_*` limits.
    pub fn new(m_cost_kib: u32, t_cost: u32, p_cost: u32) -> Result<Self, KeyError> {
        let params = Self {
            m_cost_kib,
            t_cost,
            p_cost,
        };
        params.validate()?;
        Ok(params)
    }

    /// Memory cost in KiB.
    #[must_use]
    pub fn m_cost_kib(&self) -> u32 {
        self.m_cost_kib
    }

    /// Number of passes.
    #[must_use]
    pub fn t_cost(&self) -> u32 {
        self.t_cost
    }

    /// Number of lanes.
    #[must_use]
    pub fn p_cost(&self) -> u32 {
        self.p_cost
    }

    fn validate(self) -> Result<(), KeyError> {
        let in_range = self.m_cost_kib <= Self::MAX_M_COST_KIB
            && (1..=Self::MAX_T_COST).contains(&self.t_cost)
            && (1..=Self::MAX_P_COST).contains(&self.p_cost);
        if in_range {
            self.to_argon2().map(|_| ())
        } else {
            Err(KeyError::BadParams)
        }
    }

    pub(crate) fn to_argon2(self) -> Result<argon2::Params, KeyError> {
        argon2::Params::new(
            self.m_cost_kib,
            self.t_cost,
            self.p_cost,
            Some(VaultKey::LEN),
        )
        .map_err(|_| KeyError::BadParams)
    }

    pub(crate) fn hasher(self) -> Result<Argon2<'static>, KeyError> {
        self.validate()?;
        Ok(Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            self.to_argon2()?,
        ))
    }
}

/// What a store records, in the clear, about how its key was obtained.
/// None of this is secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum KeyMetadata {
    /// A random key kept in the OS keyring.
    Keyring,
    /// A key derived from a passphrase with Argon2id.
    Passphrase {
        /// Cost parameters.
        params: Argon2Params,
        /// Random salt.
        #[serde(with = "hex::array")]
        salt: [u8; SALT_LEN],
    },
    /// A key supplied by the caller (tests; later, systemd credentials).
    External,
}

impl KeyMetadata {
    /// A short, user-facing name for the kind of key source.
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Keyring => "the OS keyring",
            Self::Passphrase { .. } => "a passphrase",
            Self::External => "an external key",
        }
    }
}

/// Supplies the key for a store.
///
/// Implementations decide how keys are created and recovered; the store
/// records the returned [`KeyMetadata`] in its unencrypted header.
pub trait KeySource {
    /// Produces a key for a new store, plus what is needed to recover it.
    ///
    /// # Errors
    /// Any [`KeyError`] from the underlying source.
    fn create_key(&self) -> Result<(VaultKey, KeyMetadata), KeyError>;

    /// Recovers the key of an existing store from its metadata.
    ///
    /// # Errors
    /// [`KeyError::Mismatch`] if `metadata` belongs to another kind of source,
    /// or any other [`KeyError`] from the underlying source.
    fn recover_key(&self, metadata: &KeyMetadata) -> Result<VaultKey, KeyError>;
}

/// A key held in memory. Used by tests, and by callers that obtain the key
/// some other way.
#[derive(Debug)]
pub struct MemoryKeySource {
    key: VaultKey,
}

impl MemoryKeySource {
    /// Uses the given key.
    #[must_use]
    pub fn new(key: VaultKey) -> Self {
        Self { key }
    }

    /// Uses a fresh random key.
    ///
    /// # Errors
    /// [`KeyError::Random`] if the OS random number generator fails.
    pub fn generate() -> Result<Self, KeyError> {
        VaultKey::generate().map(Self::new)
    }
}

impl KeySource for MemoryKeySource {
    fn create_key(&self) -> Result<(VaultKey, KeyMetadata), KeyError> {
        Ok((self.key.duplicate(), KeyMetadata::External))
    }

    fn recover_key(&self, metadata: &KeyMetadata) -> Result<VaultKey, KeyError> {
        match metadata {
            KeyMetadata::External => Ok(self.key.duplicate()),
            other => Err(KeyError::Mismatch {
                found: other.kind_name(),
                offered: KeyMetadata::External.kind_name(),
            }),
        }
    }
}

/// A key derived from a passphrase with Argon2id. A fresh salt is chosen
/// whenever a new store is created.
#[derive(Debug)]
pub struct PassphraseKeySource {
    passphrase: SecretString,
    params: Argon2Params,
}

impl PassphraseKeySource {
    /// Uses [`Argon2Params::VAULT_DEFAULT`] for new stores.
    #[must_use]
    pub fn new(passphrase: SecretString) -> Self {
        Self::with_params(passphrase, Argon2Params::VAULT_DEFAULT)
    }

    /// Uses the given parameters for new stores. Existing stores always use
    /// the parameters recorded in their header.
    #[must_use]
    pub fn with_params(passphrase: SecretString, params: Argon2Params) -> Self {
        Self { passphrase, params }
    }

    fn derive(&self, params: Argon2Params, salt: &[u8; SALT_LEN]) -> Result<VaultKey, KeyError> {
        let hasher = params.hasher()?;
        let mut outcome = Ok(());
        let key = SecretBox::init_with_mut(|k: &mut [u8; VaultKey::LEN]| {
            outcome =
                hasher.hash_password_into(self.passphrase.expose_secret().as_bytes(), salt, k);
        });
        outcome.map_err(|_| KeyError::Kdf)?;
        Ok(VaultKey(key))
    }
}

impl KeySource for PassphraseKeySource {
    fn create_key(&self) -> Result<(VaultKey, KeyMetadata), KeyError> {
        let mut salt = [0u8; SALT_LEN];
        getrandom::fill(&mut salt).map_err(|_| KeyError::Random)?;
        let key = self.derive(self.params, &salt)?;
        Ok((
            key,
            KeyMetadata::Passphrase {
                params: self.params,
                salt,
            },
        ))
    }

    fn recover_key(&self, metadata: &KeyMetadata) -> Result<VaultKey, KeyError> {
        match metadata {
            KeyMetadata::Passphrase { params, salt } => self.derive(*params, salt),
            other => Err(KeyError::Mismatch {
                found: other.kind_name(),
                offered: "a passphrase",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast() -> Argon2Params {
        Argon2Params::new(8, 1, 1).unwrap()
    }

    #[test]
    fn debug_never_shows_key_bytes() {
        let key = VaultKey::from_bytes([0xAB; 32]);
        let shown = format!("{key:?} {:?}", MemoryKeySource::new(key.duplicate()));
        assert!(!shown.to_lowercase().contains("ab, "));
        assert!(!shown.contains("171"));
        let source = PassphraseKeySource::new(SecretString::from("hunter2"));
        assert!(!format!("{source:?}").contains("hunter2"));
    }

    #[test]
    fn passphrase_recovers_the_same_key() {
        let source = PassphraseKeySource::with_params(SecretString::from("pw"), fast());
        let (key, meta) = source.create_key().unwrap();
        let again = source.recover_key(&meta).unwrap();
        assert_eq!(key.expose(), again.expose());
        let other = PassphraseKeySource::with_params(SecretString::from("nope"), fast());
        assert_ne!(other.recover_key(&meta).unwrap().expose(), key.expose());
    }

    #[test]
    fn rejects_out_of_range_params() {
        assert!(Argon2Params::new(Argon2Params::MAX_M_COST_KIB + 1, 1, 1).is_err());
        assert!(Argon2Params::new(8, 0, 1).is_err());
        assert!(Argon2Params::new(8, 1, 0).is_err());
        assert!(Argon2Params::new(1, 1, 1).is_err());
        assert!(Argon2Params::VAULT_DEFAULT.validate().is_ok());
        assert!(Argon2Params::LOGIN_DEFAULT.validate().is_ok());
    }

    #[test]
    fn sources_reject_foreign_metadata() {
        let memory = MemoryKeySource::generate().unwrap();
        assert!(matches!(
            memory.recover_key(&KeyMetadata::Keyring),
            Err(KeyError::Mismatch { .. })
        ));
        let pass = PassphraseKeySource::with_params(SecretString::from("x"), fast());
        assert!(matches!(
            pass.recover_key(&KeyMetadata::External),
            Err(KeyError::Mismatch { .. })
        ));
    }
}
