//! Reading (never writing, ADR-0014) a legacy `~/.vnc/passwd` file.
//!
//! The file holds the VncAuth password, NUL-padded to 8 bytes and encrypted
//! with single-block DES-ECB under a fixed, publicly known key. That is
//! obfuscation, not protection: anyone who can read the file can recover the
//! password. As with VncAuth itself, the DES key is the fixed key with each
//! byte's bits reversed.

use std::path::Path;

use des::Des;
use des::cipher::generic_array::GenericArray;
use des::cipher::{BlockDecrypt, KeyInit};
use secrecy::SecretString;
use zeroize::Zeroizing;

use crate::fsutil::{self, ReadError};

/// The fixed key, as conventionally written (before bit reversal).
pub const FIXED_KEY: [u8; 8] = [23, 82, 107, 6, 35, 78, 88, 7];

/// Some tools append a second 8-byte block (a view-only password); it is
/// ignored. Anything longer is not a legacy password file.
const MAX_LEN: usize = 16;

/// Why a legacy password file could not be read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LegacyPasswdError {
    /// Shorter than one 8-byte block.
    #[error("the file is too short to hold a password")]
    Truncated,
    /// Longer than any legacy password file.
    #[error("the file is {0} bytes, too long to be a VNC password file")]
    UnexpectedLength(u64),
    /// Decodes to an empty password.
    #[error("the file holds an empty password")]
    Empty,
    /// The password is not valid UTF-8.
    #[error("the password is not valid UTF-8 text")]
    NotUtf8,
    /// The file could not be read.
    #[error("could not {op} {}: {source}", path.display())]
    Io {
        /// What was being attempted.
        op: &'static str,
        /// The file involved.
        path: std::path::PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// Decodes the contents of a legacy password file. Pure; never panics.
///
/// # Errors
/// See [`LegacyPasswdError`].
pub fn decode_legacy_passwd(bytes: &[u8]) -> Result<SecretString, LegacyPasswdError> {
    if bytes.len() > MAX_LEN {
        return Err(LegacyPasswdError::UnexpectedLength(
            u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        ));
    }
    let first = bytes
        .first_chunk::<8>()
        .ok_or(LegacyPasswdError::Truncated)?;
    let mut key = [0u8; 8];
    for (k, f) in key.iter_mut().zip(FIXED_KEY) {
        *k = f.reverse_bits();
    }
    let mut block = Zeroizing::new(*first);
    Des::new(GenericArray::from_slice(&key))
        .decrypt_block(GenericArray::from_mut_slice(block.as_mut_slice()));
    let len = block.iter().position(|&b| b == 0).unwrap_or(block.len());
    let password = block.get(..len).unwrap_or_default();
    if password.is_empty() {
        return Err(LegacyPasswdError::Empty);
    }
    let text = std::str::from_utf8(password).map_err(|_| LegacyPasswdError::NotUtf8)?;
    Ok(SecretString::from(text))
}

/// Reads and decodes a legacy password file.
///
/// # Errors
/// See [`LegacyPasswdError`].
pub fn read_legacy_passwd(path: &Path) -> Result<SecretString, LegacyPasswdError> {
    let limit = u64::try_from(MAX_LEN).unwrap_or(u64::MAX);
    let bytes = fsutil::read_bounded(path, limit).map_err(|e| match e {
        ReadError::TooLarge(len) => LegacyPasswdError::UnexpectedLength(len),
        ReadError::Io(e) => LegacyPasswdError::Io {
            op: e.op,
            path: e.path,
            source: e.source,
        },
    })?;
    decode_legacy_passwd(&Zeroizing::new(bytes))
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    // Known answers from:
    //   printf 'secret12' | openssl enc -des-ecb -nopad -K E84AD660C4721AE0 \
    //     -provider legacy -provider default | xxd -p
    // where E84AD660C4721AE0 is FIXED_KEY with each byte's bits reversed,
    // derived independently in Python.
    const SECRET12: [u8; 8] = [0x24, 0xb5, 0xae, 0x4c, 0xe1, 0x55, 0x03, 0xc6];
    // printf 'abc\0\0\0\0\0' | openssl enc ... (same key)
    const ABC: [u8; 8] = [0x9c, 0x0a, 0x17, 0x2d, 0x34, 0x82, 0xe1, 0x22];
    // printf 'password' | openssl enc ... (same key)
    const PASSWORD: [u8; 8] = [0xdb, 0xd8, 0x3c, 0xfd, 0x72, 0x7a, 0x14, 0x58];

    fn decoded(bytes: &[u8]) -> String {
        decode_legacy_passwd(bytes)
            .unwrap()
            .expose_secret()
            .to_owned()
    }

    #[test]
    fn known_answers_match_openssl() {
        assert_eq!(decoded(&SECRET12), "secret12");
        assert_eq!(decoded(&ABC), "abc");
        assert_eq!(decoded(&PASSWORD), "password");
    }

    #[test]
    fn ignores_a_second_block() {
        let mut two = SECRET12.to_vec();
        two.extend_from_slice(&ABC);
        assert_eq!(decoded(&two), "secret12");
    }

    #[test]
    fn rejects_bad_lengths() {
        assert!(matches!(
            decode_legacy_passwd(&SECRET12[..7]),
            Err(LegacyPasswdError::Truncated)
        ));
        assert!(matches!(
            decode_legacy_passwd(&[0; 17]),
            Err(LegacyPasswdError::UnexpectedLength(17))
        ));
    }
}
