//! What each command does. Everything outside comes in through `Context`
//! so tests can run commands without a terminal or an OS keyring.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use farsight_vault::agent::{AgentCredentials, UserChange};
use farsight_vault::legacy::read_legacy_passwd;
use farsight_vault::{Argon2Params, KeySource, paths};
use secrecy::{ExposeSecret, SecretString};

use crate::cli::{Command, OldFile};
use crate::console::Console;

pub struct Context<'a> {
    pub credentials_path: PathBuf,
    pub keys: &'a dyn KeySource,
    pub hash_params: Argon2Params,
    pub console: &'a mut dyn Console,
    /// Normal output.
    pub out: &'a mut dyn Write,
    /// Warnings.
    pub err: &'a mut dyn Write,
}

const VNCAUTH_WARNING: &str = "\
Warning: VNC Authentication is weak. It uses only the first 8 characters of
the password, and anyone who can record the network traffic can work the
password out. Use it only for viewers that support nothing better, ideally
on a trusted network or through an SSH tunnel. Farsight's own viewer can use
a user account instead (`farsight-passwd user add <name>`), which is safer.
";

const KEYRING_HINT: &str = "could not get the key that protects the VNC Authentication password \
     from the OS keyring. Check that a keyring service (such as GNOME Keyring or KWallet) \
     is running and unlocked";

pub fn run(command: &Command, ctx: &mut Context<'_>) -> anyhow::Result<()> {
    match command {
        Command::Help | Command::Version => Ok(()),
        Command::UserAdd(name) => user_add(name, ctx),
        Command::UserRemove(name) => user_remove(name, ctx),
        Command::UserList => user_list(ctx),
        Command::VncauthSet => vncauth_set(ctx),
        Command::VncauthClear => vncauth_clear(ctx),
        Command::Import { path, old_file } => import(path.as_deref(), *old_file, ctx),
    }
}

fn load(ctx: &Context<'_>) -> anyhow::Result<AgentCredentials> {
    AgentCredentials::load_with_params(&ctx.credentials_path, ctx.hash_params).with_context(|| {
        format!(
            "could not load the credentials file {}",
            ctx.credentials_path.display()
        )
    })
}

fn save(credentials: &AgentCredentials) -> anyhow::Result<()> {
    credentials.save().with_context(|| {
        format!(
            "could not save the credentials file {}",
            credentials.path().display()
        )
    })
}

fn user_add(name: &str, ctx: &mut Context<'_>) -> anyhow::Result<()> {
    let mut credentials = load(ctx)?;
    let password = ctx
        .console
        .new_password(&format!("New password for {name}: "))?;
    let change = credentials.set_user(name, &password)?;
    save(&credentials)?;
    match change {
        UserChange::Added => writeln!(ctx.out, "Added user {name}.")?,
        UserChange::PasswordChanged => writeln!(ctx.out, "Changed the password for {name}.")?,
    }
    Ok(())
}

fn user_remove(name: &str, ctx: &mut Context<'_>) -> anyhow::Result<()> {
    let mut credentials = load(ctx)?;
    if !credentials.remove_user(name) {
        bail!("there is no user named {name:?}. Nothing was changed.");
    }
    save(&credentials)?;
    writeln!(ctx.out, "Removed user {name}.")?;
    Ok(())
}

fn user_list(ctx: &mut Context<'_>) -> anyhow::Result<()> {
    let credentials = load(ctx)?;
    let mut any = false;
    for name in credentials.usernames() {
        writeln!(ctx.out, "{name}")?;
        any = true;
    }
    if !any {
        writeln!(
            ctx.out,
            "No users yet. Add one with: farsight-passwd user add <name>"
        )?;
    }
    let state = if credentials.has_vncauth_password() {
        "set"
    } else {
        "not set"
    };
    writeln!(ctx.err, "VNC Authentication password: {state}")?;
    Ok(())
}

fn store_vncauth(
    credentials: &mut AgentCredentials,
    password: &SecretString,
    ctx: &mut Context<'_>,
) -> anyhow::Result<()> {
    if password.expose_secret().len() > 8 {
        writeln!(
            ctx.err,
            "Note: VNC Authentication uses only the first 8 characters of this password."
        )?;
    }
    if credentials.has_vncauth_password() {
        writeln!(
            ctx.err,
            "Replacing the existing VNC Authentication password."
        )?;
    }
    credentials
        .set_vncauth_password(password, ctx.keys)
        .map_err(|e| match e {
            farsight_vault::agent::AgentError::Key(key) => {
                anyhow::Error::new(key).context(KEYRING_HINT)
            }
            other => other.into(),
        })?;
    save(credentials)
}

fn vncauth_set(ctx: &mut Context<'_>) -> anyhow::Result<()> {
    write!(ctx.err, "{VNCAUTH_WARNING}")?;
    let mut credentials = load(ctx)?;
    let password = ctx
        .console
        .new_password("New VNC Authentication password: ")?;
    store_vncauth(&mut credentials, &password, ctx)?;
    writeln!(
        ctx.out,
        "The VNC Authentication password is set. It is stored sealed, never in plain text."
    )?;
    Ok(())
}

fn vncauth_clear(ctx: &mut Context<'_>) -> anyhow::Result<()> {
    let mut credentials = load(ctx)?;
    if credentials.clear_vncauth_password() {
        save(&credentials)?;
        writeln!(ctx.out, "Removed the VNC Authentication password.")?;
    } else {
        writeln!(ctx.out, "No VNC Authentication password was set.")?;
    }
    Ok(())
}

fn import(path: Option<&Path>, old_file: OldFile, ctx: &mut Context<'_>) -> anyhow::Result<()> {
    let default;
    let path = if let Some(path) = path {
        path
    } else {
        default = paths::legacy_passwd_file()
            .context("could not find your home directory; give the file's path")?;
        default.as_path()
    };
    let password =
        read_legacy_passwd(path).with_context(|| format!("could not import {}", path.display()))?;
    let mut credentials = load(ctx)?;
    store_vncauth(&mut credentials, &password, ctx)?;
    writeln!(
        ctx.out,
        "Imported the VNC Authentication password from {}.\n\
         It is now stored sealed under a key in your OS keyring.",
        path.display()
    )?;
    write!(ctx.err, "{VNCAUTH_WARNING}")?;

    let delete = match old_file {
        OldFile::Delete => true,
        OldFile::Keep => false,
        OldFile::Ask => ctx
            .console
            .confirm(&format!(
                "Delete the old file {} now? Anyone who can read it can recover the password.",
                path.display()
            ))?
            .unwrap_or(false),
    };
    if delete {
        std::fs::remove_file(path)
            .with_context(|| format!("could not delete {}", path.display()))?;
        writeln!(ctx.out, "Deleted {}.", path.display())?;
    } else {
        writeln!(
            ctx.err,
            "Kept {}. It still holds the password in a form anyone who can read the file \
             can decode. Delete it when you no longer need it.",
            path.display()
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;

    use farsight_vault::{MemoryKeySource, VaultKey};

    use super::*;

    struct Scripted {
        passwords: VecDeque<&'static str>,
        answer: Option<bool>,
    }

    impl Console for Scripted {
        fn new_password(&mut self, _prompt: &str) -> anyhow::Result<SecretString> {
            let Some(next) = self.passwords.pop_front() else {
                bail!("no more scripted passwords");
            };
            Ok(SecretString::from(next))
        }

        fn confirm(&mut self, _question: &str) -> anyhow::Result<Option<bool>> {
            Ok(self.answer)
        }
    }

    struct Harness {
        dir: tempfile::TempDir,
        keys: MemoryKeySource,
    }

    struct Output {
        result: anyhow::Result<()>,
        out: String,
        err: String,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                keys: MemoryKeySource::new(VaultKey::from_bytes([5; 32])),
            }
        }

        fn file(&self) -> PathBuf {
            self.dir.path().join("agent-credentials.toml")
        }

        fn run(
            &self,
            command: &Command,
            passwords: &[&'static str],
            answer: Option<bool>,
        ) -> Output {
            let mut console = Scripted {
                passwords: passwords.iter().copied().collect(),
                answer,
            };
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let mut ctx = Context {
                credentials_path: self.file(),
                keys: &self.keys,
                hash_params: Argon2Params::new(8, 1, 1).unwrap(),
                console: &mut console,
                out: &mut out,
                err: &mut err,
            };
            let result = run(command, &mut ctx);
            Output {
                result,
                out: String::from_utf8(out).unwrap(),
                err: String::from_utf8(err).unwrap(),
            }
        }

        fn credentials(&self) -> AgentCredentials {
            AgentCredentials::load(self.file()).unwrap()
        }
    }

    #[test]
    fn user_add_list_remove() {
        let h = Harness::new();
        let added = h.run(&Command::UserAdd("alice".into()), &["pw1"], None);
        added.result.unwrap();
        assert_eq!(added.out, "Added user alice.\n");
        assert!(h.credentials().verify("alice", b"pw1"));

        let changed = h.run(&Command::UserAdd("alice".into()), &["pw2"], None);
        assert_eq!(changed.out, "Changed the password for alice.\n");
        assert!(h.credentials().verify("alice", b"pw2"));

        let listed = h.run(&Command::UserList, &[], None);
        assert_eq!(listed.out, "alice\n");
        assert!(listed.err.contains("not set"));

        h.run(&Command::UserRemove("alice".into()), &[], None)
            .result
            .unwrap();
        assert!(
            h.run(&Command::UserRemove("alice".into()), &[], None)
                .result
                .is_err()
        );
        assert!(
            h.run(&Command::UserList, &[], None)
                .out
                .starts_with("No users yet")
        );
    }

    #[test]
    fn vncauth_set_warns_and_seals() {
        let h = Harness::new();
        let set = h.run(&Command::VncauthSet, &["longer-than-8"], None);
        set.result.unwrap();
        assert!(set.err.contains("VNC Authentication is weak"));
        assert!(set.err.contains("first 8 characters"));
        let stored = h.credentials().vncauth_password(&h.keys).unwrap().unwrap();
        assert_eq!(stored.expose_secret(), "longer-than-8");
        assert!(
            !fs::read_to_string(h.file())
                .unwrap()
                .contains("longer-than-8")
        );

        assert_eq!(
            h.run(&Command::VncauthClear, &[], None).out,
            "Removed the VNC Authentication password.\n"
        );
        assert!(!h.credentials().has_vncauth_password());
    }

    fn legacy_file(h: &Harness) -> PathBuf {
        // "secret12" under the fixed vncpasswd key (see farsight-vault's legacy tests).
        let path = h.dir.path().join("passwd");
        fs::write(&path, [0x24, 0xb5, 0xae, 0x4c, 0xe1, 0x55, 0x03, 0xc6]).unwrap();
        path
    }

    #[test]
    fn import_stores_the_password_and_deletes_on_request() {
        let h = Harness::new();
        let path = legacy_file(&h);
        let imported = h.run(
            &Command::Import {
                path: Some(path.clone()),
                old_file: OldFile::Ask,
            },
            &[],
            Some(true),
        );
        imported.result.unwrap();
        assert!(imported.out.contains("Deleted"));
        assert!(!path.exists());
        let stored = h.credentials().vncauth_password(&h.keys).unwrap().unwrap();
        assert_eq!(stored.expose_secret(), "secret12");
    }

    #[test]
    fn import_keeps_the_old_file_unless_told_otherwise() {
        let h = Harness::new();
        let path = legacy_file(&h);
        for (old_file, answer) in [
            (OldFile::Ask, None),
            (OldFile::Ask, Some(false)),
            (OldFile::Keep, Some(true)),
        ] {
            let kept = h.run(
                &Command::Import {
                    path: Some(path.clone()),
                    old_file,
                },
                &[],
                answer,
            );
            kept.result.unwrap();
            assert!(kept.err.contains("Kept"));
            assert!(path.exists());
        }
        h.run(
            &Command::Import {
                path: Some(path.clone()),
                old_file: OldFile::Delete,
            },
            &[],
            None,
        )
        .result
        .unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn import_of_a_bad_file_changes_nothing() {
        let h = Harness::new();
        let path = h.dir.path().join("short");
        fs::write(&path, [1, 2, 3]).unwrap();
        let failed = h.run(
            &Command::Import {
                path: Some(path),
                old_file: OldFile::Delete,
            },
            &[],
            None,
        );
        assert!(failed.result.is_err());
        assert!(!h.file().exists());
    }
}
