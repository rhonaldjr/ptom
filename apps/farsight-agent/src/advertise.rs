//! Announcing the agent over mDNS only on networks the user marked as theirs
//! (user decision P-32, ADR-0010 amended): elsewhere the agent still accepts
//! connections but stays quiet, so public Wi-Fi doesn't learn it exists.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use farsight_discovery::{AdvertiseConfig, Advertisement, NetworkId, advertise, current_networks};
use tracing::{info, warn};

use crate::identity::write_private;

/// File (in the agent's directory) listing trusted network ids, one per line.
const TRUSTED_FILE: &str = "trusted-networks";
/// How often to check which network the computer is on (laptops move).
const RECHECK: Duration = Duration::from_secs(30);

/// Networks the user marked as their own.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TrustedNetworks(BTreeSet<String>);

impl TrustedNetworks {
    fn path(dir: &Path) -> PathBuf {
        dir.join(TRUSTED_FILE)
    }

    /// Loads the list (empty if there is none). Invalid lines are ignored.
    pub fn load(dir: &Path) -> Self {
        let text = fs::read_to_string(Self::path(dir)).unwrap_or_default();
        Self(
            text.lines()
                .filter_map(|l| NetworkId::from_hex(l.trim()))
                .map(|n| n.as_str().to_owned())
                .collect(),
        )
    }

    /// Saves the list (owner-only file).
    pub fn save(&self, dir: &Path) -> Result<()> {
        let mut text = String::new();
        for id in &self.0 {
            text.push_str(id);
            text.push('\n');
        }
        write_private(&Self::path(dir), text.as_bytes())
    }

    /// Adds every id (not just the first new one); returns whether any was new.
    pub fn add(&mut self, ids: &[NetworkId]) -> bool {
        let mut changed = false;
        for id in ids {
            changed |= self.0.insert(id.as_str().to_owned());
        }
        changed
    }

    /// Removes every id; returns whether any was present.
    pub fn remove(&mut self, ids: &[NetworkId]) -> bool {
        let mut changed = false;
        for id in ids {
            changed |= self.0.remove(id.as_str());
        }
        changed
    }

    /// Whether any of `here` is trusted.
    pub fn contains_any(&self, here: &[NetworkId]) -> bool {
        here.iter().any(|n| self.0.contains(n.as_str()))
    }
}

/// `--trust-this-network` / `--untrust-this-network`.
pub fn change_trust(dir: &Path, trust: bool) -> Result<()> {
    let here = current_networks();
    if here.is_empty() {
        anyhow::bail!("can't identify this network (no default gateway found)");
    }
    let mut trusted = TrustedNetworks::load(dir);
    let changed = if trust {
        trusted.add(&here)
    } else {
        trusted.remove(&here)
    };
    trusted.save(dir).context("saving trusted networks")?;
    match (trust, changed) {
        (true, true) => {
            println!("This network is now trusted: the agent will announce itself here.");
        }
        (true, false) => {
            println!("This network was already trusted.");
        }
        (false, true) => {
            println!("The agent will no longer announce itself on this network.");
        }
        (false, false) => {
            println!("This network wasn't trusted.");
        }
    }
    Ok(())
}

/// Keeps the announcement in step with the current network until the agent stops.
pub async fn run(dir: PathBuf, config: AdvertiseConfig) {
    let mut current: Option<Advertisement> = None;
    loop {
        // Re-read the list too, so --trust-this-network applies without a restart.
        let trusted = TrustedNetworks::load(&dir);
        let here = current_networks();
        match (trusted.contains_any(&here), current.is_some()) {
            (true, false) => match advertise(&config) {
                Ok(announcement) => {
                    info!("announcing this computer on a trusted network");
                    current = Some(announcement);
                }
                Err(error) => warn!(%error, "could not announce this computer on the network"),
            },
            (false, true) => {
                info!("not on a trusted network: no longer announcing this computer");
                current = None; // Dropping it sends the mDNS goodbye.
            }
            _ => {}
        }
        tokio::time::sleep(RECHECK).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> NetworkId {
        NetworkId::from_hex(&format!("{byte:02x}").repeat(32)).unwrap()
    }

    #[test]
    fn trust_list_round_trips_and_matches() {
        let dir = tempfile::tempdir().unwrap();
        let mut trusted = TrustedNetworks::load(dir.path());
        assert!(!trusted.contains_any(&[id(1)]));
        assert!(trusted.add(&[id(1)]));
        assert!(!trusted.add(&[id(1)]));
        trusted.save(dir.path()).unwrap();
        let loaded = TrustedNetworks::load(dir.path());
        assert!(loaded.contains_any(&[id(2), id(1)]));
        assert!(!loaded.contains_any(&[id(2)]));
        let mut loaded = loaded;
        assert!(loaded.remove(&[id(1)]));
        assert!(!loaded.contains_any(&[id(1)]));
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(TRUSTED_FILE),
            format!("nonsense\n{}\n", id(3)),
        )
        .unwrap();
        assert_eq!(TrustedNetworks::load(dir.path()).0.len(), 1);
    }
}
