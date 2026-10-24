//! Saved computers, history, remembered credentials and pinned identities,
//! backed by the encrypted vault (ADR-0008).

use farsight_client::{Fingerprint, IdentityKind, ServerAddress};
use farsight_vault::{
    IdentityKind as PinKind, KeyringKeySource, PassphraseKeySource, PinnedIdentity, SavedServer,
    Timestamp, Vault,
};
use secrecy::{ExposeSecret, SecretString};
use tracing::warn;

/// What the vault knows about a server.
#[derive(Default)]
pub struct Known {
    pub username: Option<SecretString>,
    pub password: Option<SecretString>,
    pub pinned: Option<Fingerprint>,
}

/// A computer to show on the connect screen.
#[derive(Debug, Clone)]
pub struct Entry {
    pub label: String,
    pub address: ServerAddress,
    pub saved: bool,
}

/// Credentials to remember after a successful connection.
pub struct Remember {
    pub username: Option<SecretString>,
    pub password: SecretString,
}

pub struct Store {
    vault: Option<Vault>,
    problem: Option<String>,
    /// The keyring couldn't open the vault; a passphrase may (ADR-0008, P-8).
    locked: bool,
}

impl Store {
    /// Opens (or creates) the vault with a key from the OS keyring. If that
    /// fails, the store is *locked* until [`Self::unlock`] or [`Self::skip`].
    pub fn open() -> Self {
        let opened = Vault::default_path()
            .and_then(|path| Vault::open_or_create(path, &KeyringKeySource::vault()));
        match opened {
            Ok(vault) => Self {
                vault: Some(vault),
                problem: None,
                locked: false,
            },
            Err(error) => {
                warn!(%error, "keyring could not open saved computers");
                Self {
                    vault: None,
                    problem: Some(error.to_string()),
                    locked: true,
                }
            }
        }
    }

    /// Whether saved computers wait for a passphrase.
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Whether a vault file already exists (unlock vs. create wording).
    pub fn exists() -> bool {
        Vault::default_path().is_ok_and(|p| p.exists())
    }

    /// Opens (or creates) the vault with a passphrase (Argon2id).
    pub fn unlock(&mut self, passphrase: SecretString) -> Result<(), String> {
        let opened = Vault::default_path()
            .and_then(|path| Vault::open_or_create(path, &PassphraseKeySource::new(passphrase)));
        match opened {
            Ok(vault) => {
                self.vault = Some(vault);
                self.problem = None;
                self.locked = false;
                Ok(())
            }
            Err(error) => Err(match error {
                farsight_vault::VaultError::WrongKey => "That passphrase is not right.".to_owned(),
                other => format!("Couldn't open saved computers: {other}"),
            }),
        }
    }

    /// Continues without saved computers for this run.
    pub fn skip(&mut self) {
        self.locked = false;
        self.problem = Some("Saved computers are locked for this session. Connections still work, but nothing is remembered.".into());
    }

    /// Why the vault is unavailable, if it is.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// Saved computers and history matching `query`, saved ones first.
    pub fn search(&self, query: &str) -> Vec<Entry> {
        let Some(vault) = &self.vault else {
            return Vec::new();
        };
        let mut entries: Vec<Entry> = vault
            .search_servers(query)
            .into_iter()
            .map(|s| Entry {
                label: s.label.clone(),
                address: ServerAddress {
                    host: s.host.clone(),
                    port: s.port,
                },
                saved: true,
            })
            .collect();
        for h in vault.search_history(query) {
            let address = ServerAddress {
                host: h.host.clone(),
                port: h.port,
            };
            if !entries.iter().any(|e| e.address == address) {
                entries.push(Entry {
                    label: h.label.clone(),
                    address,
                    saved: false,
                });
            }
        }
        entries.truncate(12);
        entries
    }

    /// Credentials and pinned identity for `address`.
    pub fn lookup(&self, address: &ServerAddress) -> Known {
        let Some(server) = self.find(address) else {
            return Known::default();
        };
        Known {
            username: server.username.clone().map(SecretString::from),
            password: server.password().cloned(),
            pinned: server.pinned_identity.as_ref().map(|p| p.sha256),
        }
    }

    /// Records a successful connection: saves the computer, pins the
    /// identity it presented, and optionally remembers the credentials.
    pub fn remember(
        &mut self,
        address: &ServerAddress,
        label: &str,
        identity: Option<(IdentityKind, Fingerprint)>,
        credentials: Option<Remember>,
    ) {
        let Some(vault) = &mut self.vault else { return };
        let now = Timestamp::now();
        let id = match vault
            .servers()
            .iter()
            .find(|s| s.host == address.host && s.port == address.port)
        {
            Some(server) => server.id,
            None => match SavedServer::new(label, address.host.clone(), address.port, now) {
                Ok(server) => {
                    let id = server.id;
                    vault.upsert_server(server);
                    id
                }
                Err(error) => {
                    warn!(%error, "could not save computer");
                    return;
                }
            },
        };
        let mut result = vault.record_connection(&id, now);
        if let Some((kind, sha256)) = identity {
            let kind = match kind {
                IdentityKind::Certificate => PinKind::X509,
                IdentityKind::RsaKey => PinKind::Rsa,
            };
            result = result.and_then(|()| vault.pin_identity(&id, PinnedIdentity { kind, sha256 }));
        }
        if let Some(Remember { username, password }) = credentials {
            if let Some(server) = vault.servers().iter().find(|s| s.id == id).cloned() {
                let mut server = server;
                server.username = username.map(|u| u.expose_secret().to_owned());
                vault.upsert_server(server);
            }
            result = result.and_then(|()| vault.set_password(&id, password));
        }
        if let Err(error) = result.and_then(|()| vault.save()) {
            warn!(%error, "could not update saved computers");
        }
    }

    /// Saves a discovered computer without connecting (VWR-17). Returns
    /// false if it was already saved or the vault is unavailable.
    pub fn add(&mut self, address: &ServerAddress, label: &str) -> bool {
        if self.find(address).is_some() {
            return false;
        }
        let Some(vault) = &mut self.vault else {
            return false;
        };
        match SavedServer::new(label, address.host.clone(), address.port, Timestamp::now()) {
            Ok(server) => {
                vault.upsert_server(server);
                if let Err(error) = vault.save() {
                    warn!(%error, "could not save computer");
                }
                true
            }
            Err(error) => {
                warn!(%error, "could not save computer");
                false
            }
        }
    }

    /// Whether the user marked network `id` as their own (ADR-0012).
    pub fn is_trusted_network(&self, id: &str) -> bool {
        self.vault
            .as_ref()
            .is_some_and(|v| v.is_trusted_network(id))
    }

    /// Marks network `id` as the user's own.
    pub fn trust_network(&mut self, id: &str) {
        if let Some(vault) = &mut self.vault {
            vault.trust_network(id, "My network", Timestamp::now());
            if let Err(error) = vault.save() {
                warn!(%error, "could not save the trusted network");
            }
        }
    }

    /// Stops automatic scanning on network `id`.
    pub fn forget_network(&mut self, id: &str) {
        if let Some(vault) = &mut self.vault {
            vault.untrust_network(id);
            if let Err(error) = vault.save() {
                warn!(%error, "could not update trusted networks");
            }
        }
    }

    fn find(&self, address: &ServerAddress) -> Option<&SavedServer> {
        self.vault
            .as_ref()?
            .servers()
            .iter()
            .find(|s| s.host == address.host && s.port == address.port)
    }
}
