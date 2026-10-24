//! "Computers on your network" (M3.5, ADR-0012): passive mDNS browsing, and
//! active probing only on networks the user marked as their own.

use std::collections::BTreeMap;
use std::net::IpAddr;

use cosmic::iced::futures::{SinkExt, Stream};
use farsight_client::ServerAddress;
use farsight_discovery::{
    BrowseEvent, Browser, DiscoveredServer, ProbeConfig, ProbeEvent, ProbeScope, ServerKey, probe,
    sanitize_name,
};

/// Most discovered computers kept in memory.
const MAX_DISCOVERED: usize = 256;

/// What we know about a computer's security from discovery (advisory only:
/// a man in the middle could change it; identity is checked on connect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encryption {
    Offered,
    NotOffered,
    Unknown,
}

/// One computer to list.
#[derive(Debug, Clone)]
pub struct NearbyEntry {
    pub label: String,
    pub address: ServerAddress,
    pub encryption: Encryption,
}

/// Progress of an active scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scan {
    Running { done: usize, total: usize },
    Finished { found: usize },
}

/// Messages from the discovery streams.
#[derive(Debug, Clone)]
pub enum Discovery {
    Browse(BrowseEvent),
    Probe(ProbeEvent),
    ProbeFinished(usize),
    Unavailable(String),
}

/// Everything discovered so far, in memory only (privacy condition: saved
/// to the vault only when the user clicks Add).
#[derive(Default)]
pub struct Nearby {
    servers: BTreeMap<ServerKey, DiscoveredServer>,
    pub scan: Option<Scan>,
    pub problem: Option<String>,
}

impl Nearby {
    pub fn record(&mut self, message: Discovery) {
        match message {
            Discovery::Browse(BrowseEvent::Resolved(server))
            | Discovery::Probe(ProbeEvent::Found(server)) => {
                // Bounded: a host flooding mDNS with names can't grow this forever.
                if self.servers.len() < MAX_DISCOVERED || self.servers.contains_key(&server.key) {
                    self.servers.insert(server.key.clone(), server);
                }
            }
            Discovery::Browse(BrowseEvent::Removed(key)) => {
                self.servers.remove(&key);
            }
            Discovery::Probe(ProbeEvent::Progress(p)) => {
                self.scan = Some(Scan::Running {
                    done: p.hosts_done,
                    total: p.hosts_total,
                });
            }
            Discovery::ProbeFinished(found) => self.scan = Some(Scan::Finished { found }),
            Discovery::Unavailable(reason) => self.problem = Some(reason),
        }
    }

    /// Entries to show, one per address and port, best-informed first.
    pub fn entries(&self) -> Vec<NearbyEntry> {
        let mut by_address: BTreeMap<(String, u16), NearbyEntry> = BTreeMap::new();
        for server in self.servers.values() {
            let Some(host) = preferred_host(server) else {
                continue;
            };
            let encryption = match &server.rfb {
                Some(info) if info.offers_encryption => Encryption::Offered,
                Some(_) => Encryption::NotOffered,
                None => Encryption::Unknown,
            };
            let label = sanitize_name(&server.name);
            let entry = NearbyEntry {
                label,
                address: ServerAddress {
                    host: host.clone(),
                    port: server.port,
                },
                encryption,
            };
            by_address
                .entry((host, server.port))
                .and_modify(|existing| {
                    if existing.encryption == Encryption::Unknown {
                        existing.encryption = entry.encryption;
                    }
                    if existing.label.is_empty() {
                        existing.label.clone_from(&entry.label);
                    }
                })
                .or_insert(entry);
        }
        by_address.into_values().collect()
    }
}

fn preferred_host(server: &DiscoveredServer) -> Option<String> {
    let v4 = server.addresses.iter().find(|a| matches!(a, IpAddr::V4(_)));
    v4.or(server.addresses.first())
        .map(ToString::to_string)
        .or_else(|| server.host_name.clone())
}

/// Passive mDNS browsing for any `_rfb._tcp` server (always on).
pub fn browse() -> impl Stream<Item = Discovery> {
    cosmic::iced::stream::channel(32, async move |mut output| {
        let mut browser = match Browser::start() {
            Ok(browser) => browser,
            Err(error) => {
                let _ = output
                    .send(Discovery::Unavailable(format!(
                        "Network discovery is unavailable: {error}"
                    )))
                    .await;
                return;
            }
        };
        while let Some(event) = browser.next_event().await {
            if output.send(Discovery::Browse(event)).await.is_err() {
                return;
            }
        }
    })
}

/// One active scan of the local networks (only after consent; ADR-0012).
/// The `attempt` number makes each scan a distinct subscription.
#[allow(clippy::trivially_copy_pass_by_ref)] // `Subscription::run_with` passes `&D`.
pub fn scan(_attempt: &u64) -> impl Stream<Item = Discovery> + use<> {
    cosmic::iced::stream::channel(64, async move |mut output| {
        let scope = match ProbeScope::from_local_networks() {
            Ok(scope) => scope,
            Err(error) => {
                let _ = output
                    .send(Discovery::Unavailable(format!(
                        "Can't scan this network: {error}"
                    )))
                    .await;
                let _ = output.send(Discovery::ProbeFinished(0)).await;
                return;
            }
        };
        let (events, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let config = ProbeConfig::default();
        let scan = probe(&scope, &config, events);
        tokio::pin!(scan);
        let found = loop {
            tokio::select! {
                found = &mut scan => break found.len(),
                Some(event) = receiver.recv() => {
                    if output.send(Discovery::Probe(event)).await.is_err() {
                        return;
                    }
                }
            }
        };
        while let Ok(event) = receiver.try_recv() {
            let _ = output.send(Discovery::Probe(event)).await;
        }
        let _ = output.send(Discovery::ProbeFinished(found)).await;
    })
}
