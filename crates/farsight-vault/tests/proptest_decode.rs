//! Decoding hostile bytes never panics (CLAUDE.md: parsers are attack surface).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::OnceLock;

use farsight_vault::legacy::decode_legacy_passwd;
use farsight_vault::{MemoryKeySource, SavedServer, Timestamp, Vault, VaultKey};
use proptest::prelude::*;
use secrecy::SecretString;

fn keys() -> MemoryKeySource {
    MemoryKeySource::new(VaultKey::from_bytes([7; 32]))
}

/// A valid vault file to mutate.
fn valid_vault() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let mut vault = Vault::create(dir.path().join("v.bin"), &keys()).unwrap();
        let server =
            SavedServer::new("Lab", "lab.local", 5900, Timestamp::from_unix_secs(1)).unwrap();
        let id = server.id;
        vault.upsert_server(server);
        vault.set_password(&id, SecretString::from("pw")).unwrap();
        vault.to_bytes().unwrap()
    })
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Vault::from_bytes("unused", &bytes, &keys());
    }

    #[test]
    fn arbitrary_bytes_after_a_valid_magic_never_panic(
        kind in any::<u8>(),
        tail in proptest::collection::vec(any::<u8>(), 0..512),
    ) {
        let mut bytes = b"FSVAULT\0\x00\x01".to_vec();
        bytes.push(kind);
        bytes.extend_from_slice(&tail);
        let _ = Vault::from_bytes("unused", &bytes, &keys());
    }

    #[test]
    fn mutated_vaults_are_rejected_without_panicking(
        index in any::<prop::sample::Index>(),
        mask in 1u8..,
    ) {
        let mut bytes = valid_vault().to_vec();
        let i = index.index(bytes.len());
        bytes[i] ^= mask;
        prop_assert!(Vault::from_bytes("unused", &bytes, &keys()).is_err());
    }

    #[test]
    fn truncated_vaults_are_rejected_without_panicking(index in any::<prop::sample::Index>()) {
        let bytes = valid_vault();
        let cut = index.index(bytes.len());
        prop_assert!(Vault::from_bytes("unused", &bytes[..cut], &keys()).is_err());
    }

    #[test]
    fn legacy_decoding_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        let _ = decode_legacy_passwd(&bytes);
    }
}

#[test]
fn the_unmutated_vault_opens() {
    assert!(Vault::from_bytes("unused", valid_vault(), &keys()).is_ok());
}
