//! Acceptance tests for the viewer vault (ADR-0008).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use farsight_vault::{
    Argon2Params, HISTORY_LIMIT, IdentityKind, KeyError, MemoryKeySource, PassphraseKeySource,
    PinnedIdentity, SavedServer, Timestamp, Vault, VaultError, VaultKey,
};
use secrecy::{ExposeSecret, SecretString};

fn t(secs: u64) -> Timestamp {
    Timestamp::from_unix_secs(secs)
}

fn memory(byte: u8) -> MemoryKeySource {
    MemoryKeySource::new(VaultKey::from_bytes([byte; 32]))
}

fn fast_passphrase(text: &str) -> PassphraseKeySource {
    PassphraseKeySource::with_params(
        SecretString::from(text),
        Argon2Params::new(8, 1, 1).unwrap(),
    )
}

fn vault_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("farsight").join("vault.bin")
}

fn server(label: &str, host: &str, created: u64) -> SavedServer {
    SavedServer::new(label, host, 5900, t(created)).unwrap()
}

fn populated(path: &Path) -> Vault {
    let mut vault = Vault::create(path, &memory(1)).unwrap();
    let mut office = server("Office", "office.example.com", 10);
    office.username = Some("alice".into());
    office.view_only = true;
    let id = office.id;
    vault.upsert_server(office);
    vault
        .set_password(&id, SecretString::from("s3cret-pw"))
        .unwrap();
    vault
        .pin_identity(
            &id,
            PinnedIdentity {
                kind: IdentityKind::X509,
                sha256: [0x5a; 32],
            },
        )
        .unwrap();
    vault.record_connection(&id, t(100)).unwrap();
    vault.record_unsaved_connection("10.0.0.7", 5901, t(200));
    vault.save().unwrap();
    vault
}

#[test]
fn round_trips_everything() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let original = populated(&path);
    let id = original.servers()[0].id;

    let vault = Vault::open(&path, &memory(1)).unwrap();
    let saved = vault.server(&id).unwrap();
    assert_eq!(saved.label, "Office");
    assert_eq!(saved.host, "office.example.com");
    assert_eq!(saved.port, 5900);
    assert_eq!(saved.username.as_deref(), Some("alice"));
    assert_eq!(saved.password().unwrap().expose_secret(), "s3cret-pw");
    assert_eq!(
        saved.pinned_identity,
        Some(PinnedIdentity {
            kind: IdentityKind::X509,
            sha256: [0x5a; 32]
        })
    );
    assert!(saved.view_only);
    assert_eq!(saved.created_at, t(10));
    assert_eq!(saved.last_connected_at, Some(t(100)));
    assert_eq!(vault.history(), original.history());
    assert_eq!(vault.history().len(), 2);
}

#[test]
fn passphrase_vault_round_trips_and_rejects_a_wrong_passphrase() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let mut vault = Vault::create(&path, &fast_passphrase("correct horse")).unwrap();
    vault.upsert_server(server("Lab", "lab.local", 1));
    vault.save().unwrap();

    let reopened = Vault::open(&path, &fast_passphrase("correct horse")).unwrap();
    assert_eq!(reopened.servers().len(), 1);
    assert!(matches!(
        Vault::open(&path, &fast_passphrase("battery staple")),
        Err(VaultError::WrongKey)
    ));
}

#[test]
fn nothing_secret_is_stored_in_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let vault = populated(&path);
    let bytes = fs::read(&path).unwrap();
    for needle in [
        &b"s3cret-pw"[..],
        b"office.example.com",
        b"alice",
        b"Office",
    ] {
        assert!(!bytes.windows(needle.len()).any(|w| w == needle));
    }
    assert!(!format!("{vault:?}").contains("s3cret"));
    assert!(!format!("{:?}", vault.servers()[0]).contains("s3cret"));
}

#[test]
fn wrong_key_is_reported_as_wrong_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    populated(&path);
    assert!(matches!(
        Vault::open(&path, &memory(2)),
        Err(VaultError::WrongKey)
    ));
}

#[test]
fn a_different_kind_of_key_source_is_a_typed_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    populated(&path);
    assert!(matches!(
        Vault::open(&path, &fast_passphrase("x")),
        Err(VaultError::Key(KeyError::Mismatch { .. }))
    ));
}

#[test]
fn every_tampered_ciphertext_or_nonce_byte_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let vault = populated(&path);
    let bytes = fs::read(&path).unwrap();
    // External key source: metadata is empty, so the header is 77 bytes.
    let nonce_start = 13 + 32;
    for i in nonce_start..bytes.len() {
        if (nonce_start + 24..nonce_start + 32).contains(&i) {
            continue; // the length field; covered below
        }
        let mut copy = bytes.clone();
        copy[i] ^= 0x01;
        assert!(
            matches!(
                Vault::from_bytes(vault.path(), &copy, &memory(1)),
                Err(VaultError::Tampered)
            ),
            "byte {i}"
        );
    }
}

#[test]
fn tampered_header_fields_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    populated(&path);
    let bytes = fs::read(&path).unwrap();
    let open = |b: &[u8]| Vault::from_bytes(&path, b, &memory(1));

    let mut magic = bytes.clone();
    magic[0] ^= 0xff;
    assert!(matches!(open(&magic), Err(VaultError::NotAVault)));

    let mut version = bytes.clone();
    version[9] = 9;
    assert!(matches!(
        open(&version),
        Err(VaultError::UnsupportedVersion(9))
    ));

    let mut check = bytes.clone();
    check[20] ^= 0x01;
    assert!(matches!(open(&check), Err(VaultError::WrongKey)));

    let mut kind = bytes.clone();
    kind[10] = 1; // claims the keyring
    assert!(matches!(
        open(&kind),
        Err(VaultError::Key(KeyError::Mismatch { .. }))
    ));

    let mut length = bytes.clone();
    length[76] ^= 0x01; // lowest byte of the ciphertext length
    assert!(matches!(
        open(&length),
        Err(VaultError::Truncated | VaultError::Tampered)
    ));

    let mut appended = bytes;
    appended.push(0);
    assert!(matches!(open(&appended), Err(VaultError::Tampered)));
}

#[test]
fn every_truncation_is_reported_as_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    populated(&path);
    let bytes = fs::read(&path).unwrap();
    for cut in 0..bytes.len() {
        assert!(
            matches!(
                Vault::from_bytes(&path, &bytes[..cut], &memory(1)),
                Err(VaultError::Truncated)
            ),
            "cut at {cut}"
        );
    }
}

#[test]
fn files_are_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    populated(&path);
    let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    let dir_mode = fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600);
    assert_eq!(dir_mode, 0o700);
}

#[test]
fn saves_replace_the_file_atomically_with_a_fresh_nonce() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let mut vault = populated(&path);
    let first = fs::read(&path).unwrap();
    // A reader holding the old file keeps seeing the complete old version.
    let mut old_handle = fs::File::open(&path).unwrap();

    vault.upsert_server(server("Second", "two.example", 50));
    vault.save().unwrap();
    let second = fs::read(&path).unwrap();

    let mut through_old_handle = Vec::new();
    old_handle.read_to_end(&mut through_old_handle).unwrap();
    assert_eq!(through_old_handle, first);
    assert_ne!(first[45..69], second[45..69], "nonce must change");
    assert_eq!(Vault::open(&path, &memory(1)).unwrap().servers().len(), 2);

    // Re-saving identical contents still uses a new nonce.
    vault.save().unwrap();
    let third = fs::read(&path).unwrap();
    assert_ne!(second[45..69], third[45..69]);

    // No temporary files are left behind.
    let names: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("vault.bin")]);
}

#[test]
fn create_refuses_to_overwrite_and_open_or_create_reuses() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    populated(&path);
    assert!(matches!(
        Vault::create(&path, &memory(1)),
        Err(VaultError::AlreadyExists(_))
    ));
    let vault = Vault::open_or_create(&path, &memory(1)).unwrap();
    assert_eq!(vault.servers().len(), 1);
    let fresh = Vault::open_or_create(dir.path().join("new.bin"), &memory(1)).unwrap();
    assert!(fresh.servers().is_empty());
}

#[test]
fn oversized_files_are_refused_before_reading() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.bin");
    let file = fs::File::create(&path).unwrap();
    file.set_len(farsight_vault::MAX_VAULT_FILE_LEN + 1)
        .unwrap();
    assert!(matches!(
        Vault::open(&path, &memory(1)),
        Err(VaultError::TooLarge(_))
    ));
}

#[test]
fn missing_file_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        Vault::open(dir.path().join("nope.bin"), &memory(1)),
        Err(VaultError::Io { .. })
    ));
}

#[test]
fn search_is_case_insensitive_and_most_recent_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::create(vault_path(&dir), &memory(1)).unwrap();
    let a = server("Build Box", "build.example.com", 10);
    let b = server("Laptop", "laptop.EXAMPLE.com", 20);
    let c = server("NAS", "192.168.1.2", 30);
    let (a_id, b_id) = (a.id, b.id);
    for s in [a, b, c] {
        vault.upsert_server(s);
    }
    vault.record_connection(&a_id, t(500)).unwrap();
    vault.record_connection(&b_id, t(300)).unwrap();
    vault.record_unsaved_connection("Example.org", 5900, t(400));

    let labels =
        |found: Vec<&SavedServer>| found.iter().map(|s| s.label.clone()).collect::<Vec<_>>();
    assert_eq!(
        labels(vault.search_servers("example")),
        ["Build Box", "Laptop"]
    );
    assert_eq!(labels(vault.search_servers("BOX")), ["Build Box"]);
    assert_eq!(
        labels(vault.search_servers("")),
        ["Build Box", "Laptop", "NAS"]
    );
    assert!(vault.search_servers("nothing").is_empty());

    let hosts: Vec<_> = vault
        .search_history("EXAMPLE")
        .iter()
        .map(|e| e.host.clone())
        .collect();
    assert_eq!(
        hosts,
        ["build.example.com", "Example.org", "laptop.EXAMPLE.com"]
    );
    assert_eq!(vault.search_history("laptop").len(), 1);

    vault.clear_history();
    assert_eq!(vault.history().len(), 0);
    assert_eq!(vault.search_history("").len(), 0);
    // Saved servers keep their last connection time.
    assert_eq!(vault.server(&a_id).unwrap().last_connected_at, Some(t(500)));
}

#[test]
fn equal_times_list_the_later_record_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::create(vault_path(&dir), &memory(1)).unwrap();
    vault.record_unsaved_connection("first", 1, t(5));
    vault.record_unsaved_connection("second", 2, t(5));
    let hosts: Vec<_> = vault
        .search_history("")
        .iter()
        .map(|e| e.host.clone())
        .collect();
    assert_eq!(hosts, ["second", "first"]);
}

#[test]
fn server_edits_and_removal() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::create(vault_path(&dir), &memory(1)).unwrap();
    let s = server("Pi", "pi.local", 1);
    let id = s.id;
    assert!(vault.upsert_server(s.clone()).is_none());

    let mut renamed = s;
    renamed.label = "Raspberry Pi".into();
    assert_eq!(vault.upsert_server(renamed).unwrap().label, "Pi");
    assert_eq!(vault.servers().len(), 1);

    vault.set_password(&id, SecretString::from("pw")).unwrap();
    vault.clear_password(&id).unwrap();
    assert!(vault.server(&id).unwrap().password().is_none());
    vault
        .pin_identity(
            &id,
            PinnedIdentity {
                kind: IdentityKind::Rsa,
                sha256: [1; 32],
            },
        )
        .unwrap();
    vault.unpin_identity(&id).unwrap();
    assert!(vault.server(&id).unwrap().pinned_identity.is_none());

    vault.record_connection(&id, t(9)).unwrap();
    assert!(vault.remove_server(&id).is_some());
    assert!(vault.remove_server(&id).is_none());
    assert_eq!(vault.history()[0].server_id, None, "history is unlinked");
    assert!(matches!(
        vault.set_password(&id, SecretString::from("x")),
        Err(VaultError::UnknownServer(_))
    ));
    assert!(matches!(
        vault.record_connection(&id, t(10)),
        Err(VaultError::UnknownServer(_))
    ));
}

#[test]
fn history_is_capped() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = Vault::create(vault_path(&dir), &memory(1)).unwrap();
    for i in 0..(HISTORY_LIMIT as u64 + 5) {
        vault.record_unsaved_connection(format!("h{i}"), 1, t(i));
    }
    assert_eq!(vault.history().len(), HISTORY_LIMIT);
    assert_eq!(vault.history()[0].host, "h5");
}

#[test]
fn server_ids_round_trip_as_text() {
    let id = server("x", "y", 0).id;
    let text = id.to_string();
    assert_eq!(text.len(), 32);
    assert_eq!(text.parse::<farsight_vault::ServerId>().unwrap(), id);
    assert!("xyz".parse::<farsight_vault::ServerId>().is_err());
}
