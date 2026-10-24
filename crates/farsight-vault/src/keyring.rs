//! A random 256-bit key kept in the OS keyring: Secret Service on Linux
//! (over zbus, pure Rust) and the Keychain on macOS.

use zeroize::Zeroizing;

use crate::error::KeyError;
use crate::key::{KeyMetadata, KeySource, VaultKey};

/// Keyring service name shared by all Farsight entries.
pub const SERVICE: &str = "farsight";

/// Stores the key as a binary secret under `(service, account)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyringKeySource {
    service: String,
    account: String,
}

impl KeyringKeySource {
    /// Any service and account name.
    #[must_use]
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    /// The entry holding the viewer vault's key.
    #[must_use]
    pub fn vault() -> Self {
        Self::new(SERVICE, "vault-key")
    }

    /// The entry holding the key that seals the agent's VncAuth password.
    #[must_use]
    pub fn agent() -> Self {
        Self::new(SERVICE, "agent-vncauth-key")
    }

    fn entry(&self) -> Result<keyring_core::Entry, KeyError> {
        platform::entry(&self.service, &self.account)
    }
}

impl KeySource for KeyringKeySource {
    /// Reuses the key already in this entry, if any, so that creating a new
    /// store never orphans another store sealed under the same entry.
    fn create_key(&self) -> Result<(VaultKey, KeyMetadata), KeyError> {
        let entry = self.entry()?;
        match entry.get_secret() {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                Ok((VaultKey::from_slice(&bytes)?, KeyMetadata::Keyring))
            }
            Err(keyring_core::Error::NoEntry) => {
                let key = VaultKey::generate()?;
                entry.set_secret(key.expose()).map_err(map_error)?;
                Ok((key, KeyMetadata::Keyring))
            }
            Err(other) => Err(map_error(other)),
        }
    }

    fn recover_key(&self, metadata: &KeyMetadata) -> Result<VaultKey, KeyError> {
        if *metadata != KeyMetadata::Keyring {
            return Err(KeyError::Mismatch {
                found: metadata.kind_name(),
                offered: KeyMetadata::Keyring.kind_name(),
            });
        }
        let bytes = Zeroizing::new(self.entry()?.get_secret().map_err(map_error)?);
        VaultKey::from_slice(&bytes)
    }
}

/// Maps keyring errors without carrying any secret payload along.
fn map_error(error: keyring_core::Error) -> KeyError {
    use keyring_core::Error as E;
    match error {
        E::NoEntry => KeyError::NotFound,
        E::NoStorageAccess(platform) => KeyError::Unavailable(platform.to_string()),
        E::PlatformFailure(platform) => KeyError::Keyring(platform.to_string()),
        E::BadEncoding(_) | E::BadDataFormat(..) => KeyError::Malformed,
        E::Ambiguous(_) => KeyError::Keyring("more than one matching keyring entry".into()),
        _ => KeyError::Keyring("unexpected keyring error".into()),
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use keyring_core::api::CredentialStoreApi;

    use super::map_error;
    use crate::error::KeyError;

    pub(super) fn entry(service: &str, account: &str) -> Result<keyring_core::Entry, KeyError> {
        let store = zbus_secret_service_keyring_store::Store::new().map_err(map_error)?;
        store.build(service, account, None).map_err(map_error)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use keyring_core::api::CredentialStoreApi;

    use super::map_error;
    use crate::error::KeyError;

    pub(super) fn entry(service: &str, account: &str) -> Result<keyring_core::Entry, KeyError> {
        let store = apple_native_keyring_store::keychain::Store::new().map_err(map_error)?;
        store.build(service, account, None).map_err(map_error)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use crate::error::KeyError;

    pub(super) fn entry(_service: &str, _account: &str) -> Result<keyring_core::Entry, KeyError> {
        Err(KeyError::Unsupported)
    }
}
