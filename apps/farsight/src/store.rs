//! Saved computers, history, remembered credentials and pinned identities,
//! backed by the encrypted vault (ADR-0008).

use farsight_client::{Fingerprint, IdentityKind, ServerAddress};
use farsight_vault::{
    IdentityKind as PinKind, KeyringKeySource, PinnedIdentity, SavedServer, Timestamp, Vault,
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
}

impl Store {
    /// Opens (or creates) the vault with a key from the OS keyring.
    pub fn open() -> Self {
        let opened = Vault::default_path()
            .and_then(|path| Vault::open_or_create(path, &KeyringKeySource::vault()));
        match opened {
            Ok(vault) => Self {
                vault: Some(vault),
                problem: None,
            },
            Err(error) => {
                warn!(%error, "saved computers unavailable");
                Self {
                    vault: None,
                    problem: Some(format!(
                        "Saved computers are unavailable ({error}). Connections still work, but nothing is remembered."
                    )),
                }
            }
        }
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

    fn find(&self, address: &ServerAddress) -> Option<&SavedServer> {
        self.vault
            .as_ref()?
            .servers()
            .iter()
            .find(|s| s.host == address.host && s.port == address.port)
    }
}
