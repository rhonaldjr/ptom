//! XChaCha20-Poly1305 sealing with a key-check value.
//!
//! The master key is never used directly. HKDF-SHA256 derives two values
//! from it, bound to a domain string:
//! - the AEAD key, and
//! - a public *key check*, stored next to the ciphertext. A mismatch means
//!   the wrong key was offered, which lets callers tell "wrong key" apart
//!   from "tampered data". It also commits the ciphertext to one key.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::error::KeyError;
use crate::key::VaultKey;

/// Nonce length for XChaCha20-Poly1305.
pub(crate) const NONCE_LEN: usize = 24;
/// Poly1305 tag length.
pub(crate) const TAG_LEN: usize = 16;
/// Key-check length.
pub(crate) const CHECK_LEN: usize = 32;

pub(crate) struct DerivedKeys {
    aead: Zeroizing<[u8; 32]>,
    check: [u8; CHECK_LEN],
}

/// Why opening failed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OpenError {
    WrongKey,
    Tampered,
}

impl DerivedKeys {
    pub(crate) fn derive(master: &VaultKey, domain: &str) -> Self {
        let hk = Hkdf::<Sha256>::new(None, master.expose());
        let mut aead = Zeroizing::new([0u8; 32]);
        let mut check = [0u8; CHECK_LEN];
        // Both outputs are far below HKDF-SHA256's 8160-byte limit, so the
        // expansions cannot fail; ignore the impossible error.
        let _ = hk.expand_multi_info(&[domain.as_bytes(), b"/aead-key"], aead.as_mut());
        let _ = hk.expand_multi_info(&[domain.as_bytes(), b"/key-check"], &mut check);
        Self { aead, check }
    }

    pub(crate) fn check(&self) -> &[u8; CHECK_LEN] {
        &self.check
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(&Key::from(*self.aead))
    }

    /// Returns `None` only if the message is too long for the cipher
    /// (about 256 GiB).
    pub(crate) fn seal(
        &self,
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        plaintext: &[u8],
    ) -> Option<Vec<u8>> {
        self.cipher()
            .encrypt(
                &XNonce::from(*nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .ok()
    }

    pub(crate) fn open(
        &self,
        expected_check: &[u8; CHECK_LEN],
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        ciphertext: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, OpenError> {
        if !bool::from(self.check.ct_eq(expected_check)) {
            return Err(OpenError::WrongKey);
        }
        self.cipher()
            .decrypt(
                &XNonce::from(*nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| OpenError::Tampered)
    }
}

pub(crate) fn random_nonce() -> Result<[u8; NONCE_LEN], KeyError> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|_| KeyError::Random)?;
    Ok(nonce)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_and_failure_modes() {
        let keys = DerivedKeys::derive(&VaultKey::from_bytes([1; 32]), "test");
        let nonce = [2; NONCE_LEN];
        let sealed = keys.seal(&nonce, b"aad", b"hello").unwrap();
        let opened = keys.open(keys.check(), &nonce, b"aad", &sealed).unwrap();
        assert_eq!(opened.as_slice(), b"hello");

        let mut bad = sealed.clone();
        bad[0] ^= 1;
        assert_eq!(
            keys.open(keys.check(), &nonce, b"aad", &bad).unwrap_err(),
            OpenError::Tampered
        );
        assert_eq!(
            keys.open(keys.check(), &nonce, b"other", &sealed)
                .unwrap_err(),
            OpenError::Tampered
        );

        let other = DerivedKeys::derive(&VaultKey::from_bytes([9; 32]), "test");
        assert_eq!(
            other
                .open(keys.check(), &nonce, b"aad", &sealed)
                .unwrap_err(),
            OpenError::WrongKey
        );
    }

    #[test]
    fn domains_separate_keys() {
        let master = VaultKey::from_bytes([1; 32]);
        let a = DerivedKeys::derive(&master, "a");
        let b = DerivedKeys::derive(&master, "b");
        assert_ne!(a.check(), b.check());
        assert_ne!(*a.aead, *b.aead);
    }
}
