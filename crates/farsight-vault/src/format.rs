//! The vault file layout. All integers are big-endian.
//!
//! ```text
//! offset  size  field
//! 0       8     magic "FSVAULT\0"
//! 8       2     format version (1)
//! 10      1     key source: 1 = OS keyring, 2 = Argon2id passphrase, 3 = external
//! 11      2     key-source metadata length M
//! 13      M     metadata (passphrase: m_cost KiB u32, t_cost u32, p_cost u32, salt[16]; others: empty)
//! 13+M    32    key check (HKDF-SHA256 of the master key, see `crypto`)
//! 45+M    24    XChaCha20-Poly1305 nonce, fresh for every write
//! 69+M    8     ciphertext length C (includes the 16-byte tag)
//! 77+M    C     ciphertext of the JSON-encoded contents
//! ```
//!
//! Everything before the ciphertext is the AEAD's associated data, so any
//! change to the header is detected.

use crate::crypto::{CHECK_LEN, NONCE_LEN, TAG_LEN};
use crate::error::VaultError;
use crate::key::{Argon2Params, KeyMetadata, SALT_LEN};

/// Identifies a vault file.
pub(crate) const MAGIC: [u8; 8] = *b"FSVAULT\0";
/// The only format version so far.
pub(crate) const VERSION: u16 = 1;

const KIND_KEYRING: u8 = 1;
const KIND_PASSPHRASE: u8 = 2;
const KIND_EXTERNAL: u8 = 3;
const PASSPHRASE_META_LEN: usize = 12 + SALT_LEN;

/// The parts of the header that callers need.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) metadata: KeyMetadata,
    pub(crate) check: [u8; CHECK_LEN],
    pub(crate) nonce: [u8; NONCE_LEN],
}

/// A parsed file: header, the bytes it was parsed from (the AAD), and the
/// ciphertext.
#[derive(Debug)]
pub(crate) struct Parsed<'a> {
    pub(crate) header: Header,
    pub(crate) aad: &'a [u8],
    pub(crate) ciphertext: &'a [u8],
}

/// Encodes the header (the AAD) for a ciphertext of `ciphertext_len` bytes.
pub(crate) fn encode_header(header: &Header, ciphertext_len: u64) -> Vec<u8> {
    let meta = encode_metadata(&header.metadata);
    let mut out = Vec::with_capacity(13 + meta.len() + CHECK_LEN + NONCE_LEN + 8);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.push(kind_byte(&header.metadata));
    // Metadata is at most PASSPHRASE_META_LEN bytes, so this always fits.
    out.extend_from_slice(&u16::try_from(meta.len()).unwrap_or(u16::MAX).to_be_bytes());
    out.extend_from_slice(&meta);
    out.extend_from_slice(&header.check);
    out.extend_from_slice(&header.nonce);
    out.extend_from_slice(&ciphertext_len.to_be_bytes());
    out
}

/// Parses a whole file. Never panics, whatever the input.
pub(crate) fn parse(bytes: &[u8]) -> Result<Parsed<'_>, VaultError> {
    let Some(magic) = bytes.first_chunk::<8>() else {
        return Err(if MAGIC.starts_with(bytes) {
            VaultError::Truncated
        } else {
            VaultError::NotAVault
        });
    };
    if *magic != MAGIC {
        return Err(VaultError::NotAVault);
    }
    let mut reader = Reader {
        rest: bytes.get(MAGIC.len()..).unwrap_or_default(),
    };
    let version = u16::from_be_bytes(reader.array()?);
    if version != VERSION {
        return Err(VaultError::UnsupportedVersion(version));
    }
    let [kind] = reader.array()?;
    let meta_len = usize::from(u16::from_be_bytes(reader.array()?));
    let metadata = decode_metadata(kind, reader.take(meta_len)?)?;
    let check = reader.array()?;
    let nonce = reader.array()?;
    let declared = u64::from_be_bytes(reader.array()?);
    let aad_len = bytes.len() - reader.rest.len();
    let aad = bytes.get(..aad_len).ok_or(VaultError::Truncated)?;
    let ciphertext_len = usize::try_from(declared).map_err(|_| VaultError::Truncated)?;
    if ciphertext_len < TAG_LEN {
        return Err(VaultError::MalformedHeader(
            "ciphertext length is too small",
        ));
    }
    let ciphertext = reader.take(ciphertext_len)?;
    if !reader.rest.is_empty() {
        // Bytes after the authenticated ciphertext: someone appended data.
        return Err(VaultError::Tampered);
    }
    Ok(Parsed {
        header: Header {
            metadata,
            check,
            nonce,
        },
        aad,
        ciphertext,
    })
}

fn kind_byte(metadata: &KeyMetadata) -> u8 {
    match metadata {
        KeyMetadata::Keyring => KIND_KEYRING,
        KeyMetadata::Passphrase { .. } => KIND_PASSPHRASE,
        KeyMetadata::External => KIND_EXTERNAL,
    }
}

fn encode_metadata(metadata: &KeyMetadata) -> Vec<u8> {
    match metadata {
        KeyMetadata::Keyring | KeyMetadata::External => Vec::new(),
        KeyMetadata::Passphrase { params, salt } => {
            let mut out = Vec::with_capacity(PASSPHRASE_META_LEN);
            out.extend_from_slice(&params.m_cost_kib().to_be_bytes());
            out.extend_from_slice(&params.t_cost().to_be_bytes());
            out.extend_from_slice(&params.p_cost().to_be_bytes());
            out.extend_from_slice(salt);
            out
        }
    }
}

fn decode_metadata(kind: u8, meta: &[u8]) -> Result<KeyMetadata, VaultError> {
    let expect_empty = |metadata| {
        if meta.is_empty() {
            Ok(metadata)
        } else {
            Err(VaultError::MalformedHeader(
                "unexpected key-source metadata",
            ))
        }
    };
    match kind {
        KIND_KEYRING => expect_empty(KeyMetadata::Keyring),
        KIND_EXTERNAL => expect_empty(KeyMetadata::External),
        KIND_PASSPHRASE => {
            if meta.len() != PASSPHRASE_META_LEN {
                return Err(VaultError::MalformedHeader(
                    "wrong passphrase metadata length",
                ));
            }
            let mut reader = Reader { rest: meta };
            let m = u32::from_be_bytes(reader.array()?);
            let t = u32::from_be_bytes(reader.array()?);
            let p = u32::from_be_bytes(reader.array()?);
            let salt = reader.array()?;
            let params = Argon2Params::new(m, t, p)
                .map_err(|_| VaultError::MalformedHeader("Argon2 parameters out of range"))?;
            Ok(KeyMetadata::Passphrase { params, salt })
        }
        _ => Err(VaultError::MalformedHeader("unknown key source")),
    }
}

struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], VaultError> {
        let (head, tail) = self.rest.split_at_checked(n).ok_or(VaultError::Truncated)?;
        self.rest = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], VaultError> {
        let (head, tail) = self
            .rest
            .split_first_chunk::<N>()
            .ok_or(VaultError::Truncated)?;
        self.rest = tail;
        Ok(*head)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(metadata: KeyMetadata) -> Vec<u8> {
        let header = Header {
            metadata,
            check: [7; CHECK_LEN],
            nonce: [8; NONCE_LEN],
        };
        let mut file = encode_header(&header, 20);
        file.extend_from_slice(&[0xCC; 20]);
        file
    }

    #[test]
    fn round_trips_every_key_source() {
        let passphrase = KeyMetadata::Passphrase {
            params: Argon2Params::new(64, 2, 1).unwrap(),
            salt: [3; SALT_LEN],
        };
        for metadata in [KeyMetadata::Keyring, KeyMetadata::External, passphrase] {
            let file = sample(metadata.clone());
            let parsed = parse(&file).unwrap();
            assert_eq!(parsed.header.metadata, metadata);
            assert_eq!(parsed.header.nonce, [8; NONCE_LEN]);
            assert_eq!(parsed.ciphertext, &[0xCC; 20]);
            assert_eq!(parsed.aad.len() + 20, file.len());
        }
    }

    #[test]
    fn distinguishes_failure_modes() {
        let file = sample(KeyMetadata::Keyring);
        assert!(matches!(parse(b""), Err(VaultError::Truncated)));
        assert!(matches!(parse(b"FSVA"), Err(VaultError::Truncated)));
        assert!(matches!(
            parse(b"hello world, not a vault"),
            Err(VaultError::NotAVault)
        ));
        for cut in 1..file.len() {
            assert!(
                matches!(parse(&file[..cut]), Err(VaultError::Truncated)),
                "cut at {cut}"
            );
        }
        let mut longer = file.clone();
        longer.push(0);
        assert!(matches!(parse(&longer), Err(VaultError::Tampered)));
        let mut future = file.clone();
        future[9] = 2;
        assert!(matches!(
            parse(&future),
            Err(VaultError::UnsupportedVersion(2))
        ));
        let mut unknown = file;
        unknown[10] = 99;
        assert!(matches!(
            parse(&unknown),
            Err(VaultError::MalformedHeader(_))
        ));
    }
}
