//! Acceptance tests for agent credentials (ADR-0014).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use farsight_vault::agent::{AgentCredentials, AgentError, UserChange};
use farsight_vault::{Argon2Params, KeyError, MemoryKeySource, PassphraseKeySource, VaultKey};
use secrecy::{ExposeSecret, SecretString};

fn fast() -> Argon2Params {
    Argon2Params::new(8, 1, 1).unwrap()
}

fn path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("farsight").join("agent-credentials.toml")
}

fn load(dir: &tempfile::TempDir) -> AgentCredentials {
    AgentCredentials::load_with_params(path(dir), fast()).unwrap()
}

fn pw(text: &str) -> SecretString {
    SecretString::from(text)
}

fn memory(byte: u8) -> MemoryKeySource {
    MemoryKeySource::new(VaultKey::from_bytes([byte; 32]))
}

#[test]
fn verify_accepts_only_the_right_user_and_password() {
    let dir = tempfile::tempdir().unwrap();
    let mut creds = load(&dir);
    assert_eq!(
        creds.set_user("alice", &pw("wonderland")).unwrap(),
        UserChange::Added
    );
    creds.set_user("bob", &pw("builder")).unwrap();
    creds.save().unwrap();

    let creds = load(&dir);
    assert!(creds.verify("alice", b"wonderland"));
    assert!(creds.verify("bob", b"builder"));
    assert!(!creds.verify("alice", b"builder"), "wrong password");
    assert!(!creds.verify("alice", b""), "empty password");
    assert!(!creds.verify("mallory", b"wonderland"), "unknown user");
    assert!(!creds.verify("Alice", b"wonderland"), "usernames are exact");
    assert_eq!(creds.usernames().collect::<Vec<_>>(), ["alice", "bob"]);
}

#[test]
fn changing_and_removing_users() {
    let dir = tempfile::tempdir().unwrap();
    let mut creds = load(&dir);
    creds.set_user("alice", &pw("one")).unwrap();
    assert_eq!(
        creds.set_user("alice", &pw("two")).unwrap(),
        UserChange::PasswordChanged
    );
    assert!(!creds.verify("alice", b"one"));
    assert!(creds.verify("alice", b"two"));
    assert!(creds.remove_user("alice"));
    assert!(!creds.remove_user("alice"));
    assert!(!creds.verify("alice", b"two"));
    assert!(!creds.has_user("alice"));
}

#[test]
fn rejects_bad_usernames_and_passwords() {
    let dir = tempfile::tempdir().unwrap();
    let mut creds = load(&dir);
    for name in ["", " alice", "bob ", "a\nb", &"x".repeat(256)] {
        assert!(
            matches!(
                creds.set_user(name, &pw("pw")),
                Err(AgentError::InvalidUsername(_))
            ),
            "{name:?}"
        );
    }
    assert!(matches!(
        creds.set_user("alice", &pw("")),
        Err(AgentError::InvalidPassword(_))
    ));
}

#[test]
fn file_holds_hashes_only_and_is_private() {
    let dir = tempfile::tempdir().unwrap();
    let mut creds = load(&dir);
    creds.set_user("alice", &pw("wonderland")).unwrap();
    creds
        .set_vncauth_password(&pw("legacy12"), &memory(3))
        .unwrap();
    creds.save().unwrap();

    let text = fs::read_to_string(path(&dir)).unwrap();
    assert!(text.contains("$argon2id$"));
    assert!(!text.contains("wonderland"));
    assert!(!text.contains("legacy12"));
    let mode = fs::metadata(path(&dir)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let dir_mode = fs::metadata(path(&dir).parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
    assert!(!format!("{creds:?}").contains("argon2"));
}

#[test]
fn vncauth_password_round_trips_and_clears() {
    let dir = tempfile::tempdir().unwrap();
    let mut creds = load(&dir);
    assert!(creds.vncauth_password(&memory(3)).unwrap().is_none());
    creds
        .set_vncauth_password(&pw("legacy12"), &memory(3))
        .unwrap();
    creds.save().unwrap();

    let mut creds = load(&dir);
    assert!(creds.has_vncauth_password());
    let password = creds.vncauth_password(&memory(3)).unwrap().unwrap();
    assert_eq!(password.expose_secret(), "legacy12");

    assert!(creds.clear_vncauth_password());
    creds.save().unwrap();
    assert!(!load(&dir).has_vncauth_password());
}

#[test]
fn vncauth_password_wrong_key_and_tampering_are_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let mut creds = load(&dir);
    creds
        .set_vncauth_password(&pw("legacy12"), &memory(3))
        .unwrap();
    creds.save().unwrap();

    assert!(matches!(
        load(&dir).vncauth_password(&memory(4)),
        Err(AgentError::WrongKey)
    ));
    let passphrase = PassphraseKeySource::with_params(pw("x"), fast());
    assert!(matches!(
        load(&dir).vncauth_password(&passphrase),
        Err(AgentError::Key(KeyError::Mismatch { .. }))
    ));

    // Flip one hex digit of the ciphertext.
    let text = fs::read_to_string(path(&dir)).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("ciphertext"))
        .unwrap()
        .to_owned();
    let flipped = if line.ends_with("0\"") {
        line.replacen("0\"", "1\"", 1)
    } else {
        format!("{}0\"", &line[..line.len() - 2])
    };
    fs::write(path(&dir), text.replace(&line, &flipped)).unwrap();
    assert!(matches!(
        load(&dir).vncauth_password(&memory(3)),
        Err(AgentError::Tampered)
    ));
}

#[test]
fn malformed_files_are_typed_errors() {
    let dir = tempfile::tempdir().unwrap();
    let file = path(&dir);
    fs::create_dir_all(file.parent().unwrap()).unwrap();

    fs::write(&file, "version = 2\n").unwrap();
    assert!(matches!(
        AgentCredentials::load(&file),
        Err(AgentError::UnsupportedVersion(2))
    ));

    fs::write(&file, "this is not toml = = =").unwrap();
    assert!(matches!(
        AgentCredentials::load(&file),
        Err(AgentError::Parse(_))
    ));

    fs::write(
        &file,
        "version = 1\n[[user]]\nname = \"a\"\nhash = \"x\"\n[[user]]\nname = \"a\"\nhash = \"y\"\n",
    )
    .unwrap();
    assert!(matches!(
        AgentCredentials::load(&file),
        Err(AgentError::DuplicateUser(_))
    ));

    // A malformed stored hash never verifies.
    fs::write(
        &file,
        "version = 1\n[[user]]\nname = \"a\"\nhash = \"not-a-phc\"\n",
    )
    .unwrap();
    let creds = AgentCredentials::load_with_params(&file, fast()).unwrap();
    assert!(!creds.verify("a", b"anything"));
}

#[test]
fn missing_file_loads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let creds = load(&dir);
    assert_eq!(creds.usernames().count(), 0);
    assert!(!creds.has_vncauth_password());
    assert!(!creds.verify("anyone", b"anything"));
}
