//! Identity of the network the machine is on, for the "This is my network"
//! trust list (ADR-0012).
//!
//! A network is identified by its default gateway's MAC address plus the
//! subnet the gateway is on. The identity is hashed so the trust list does not
//! store hardware addresses. It is a convenience, not a security boundary: a
//! hostile network can copy both values.

use std::fmt;
use std::net::Ipv4Addr;

use sha2::{Digest, Sha256};

use crate::procfs::{ArpEntry, Route, parse_arp, parse_routes, prefix_len};

/// A network's identity: lowercase hex SHA-256 of
/// `"<gateway mac>|<network>/<prefix>"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NetworkId(String);

impl NetworkId {
    /// Computes the identity from a gateway MAC (any case) and its subnet.
    #[must_use]
    pub fn new(gateway_mac: &str, network: Ipv4Addr, prefix: u8) -> Self {
        let input = format!("{}|{network}/{prefix}", gateway_mac.to_ascii_lowercase());
        let digest = Sha256::digest(input.as_bytes());
        Self(
            digest
                .iter()
                .fold(String::with_capacity(64), |mut hex, byte| {
                    use fmt::Write as _;
                    let _ = write!(hex, "{byte:02x}");
                    hex
                }),
        )
    }

    /// Wraps a stored identity (for example one read back from the vault).
    /// Returns `None` unless it is 64 lowercase hex digits.
    #[must_use]
    pub fn from_hex(hex: &str) -> Option<Self> {
        (hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .then(|| Self(hex.to_owned()))
    }

    /// The identity as lowercase hex.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NetworkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The identities of the networks this machine currently reaches through a
/// default gateway. Linux reads `/proc/net/route` and `/proc/net/arp`; other
/// platforms return an empty list (so probing never runs automatically there).
#[must_use]
pub fn current_networks() -> Vec<NetworkId> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    let read = |path| std::fs::read_to_string(path).unwrap_or_default();
    networks_from_proc(&read("/proc/net/route"), &read("/proc/net/arp"))
}

/// Computes network identities from the contents of `/proc/net/route` and
/// `/proc/net/arp`. A default route counts only if its gateway's MAC is known
/// and an on-link route on the same interface contains the gateway.
#[must_use]
pub fn networks_from_proc(route_text: &str, arp_text: &str) -> Vec<NetworkId> {
    let routes = parse_routes(route_text);
    let neighbours = parse_arp(arp_text);
    let mut ids: Vec<NetworkId> = routes
        .iter()
        .filter(|r| r.is_default())
        .filter_map(|default| network_id(default, &routes, &neighbours))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn network_id(default: &Route, routes: &[Route], neighbours: &[ArpEntry]) -> Option<NetworkId> {
    let gateway = default.gateway;
    let subnet = routes
        .iter()
        .filter(|r| r.interface == default.interface && r.is_on_link() && r.contains(gateway))
        .filter_map(|r| prefix_len(r.mask).map(|p| (r, p)))
        .max_by_key(|(_, prefix)| *prefix)?;
    let mac = neighbours
        .iter()
        .find(|n| n.ip == gateway && n.interface == default.interface)?;
    let (route, prefix) = subnet;
    let network = Ipv4Addr::from(u32::from(route.destination) & u32::from(route.mask));
    Some(NetworkId::new(&mac.mac, network, prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procfs::fixtures::{ARP, ROUTE};

    #[test]
    fn id_is_stable_and_case_insensitive() {
        let a = NetworkId::new("AA:BB:CC:DD:EE:01", Ipv4Addr::new(192, 168, 1, 0), 24);
        let b = NetworkId::new("aa:bb:cc:dd:ee:01", Ipv4Addr::new(192, 168, 1, 0), 24);
        assert_eq!(a, b);
        assert_eq!(a.as_str().len(), 64);
        assert_eq!(NetworkId::from_hex(a.as_str()), Some(a.clone()));
        let other_subnet = NetworkId::new("aa:bb:cc:dd:ee:01", Ipv4Addr::new(192, 168, 2, 0), 24);
        assert_ne!(a, other_subnet);
    }

    #[test]
    fn known_vector() {
        // echo -n 'aa:bb:cc:dd:ee:01|192.168.1.0/24' | sha256sum
        let id = NetworkId::new("aa:bb:cc:dd:ee:01", Ipv4Addr::new(192, 168, 1, 0), 24);
        assert_eq!(id.as_str(), KNOWN);
    }

    const KNOWN: &str = "23d58df737315075d00a9b20a673a9533cf58e0c75e2e7307ee09e2007566064";

    #[cfg(target_endian = "little")]
    #[test]
    fn derives_id_from_proc_fixtures() {
        let ids = networks_from_proc(ROUTE, ARP);
        assert_eq!(
            ids,
            vec![NetworkId::new(
                "aa:bb:cc:dd:ee:01",
                Ipv4Addr::new(192, 168, 1, 0),
                24
            )]
        );
    }

    #[cfg(target_endian = "little")]
    #[test]
    fn unknown_gateway_mac_gives_no_id() {
        let arp_without_gateway = "IP address HW type Flags HW address Mask Device\n";
        assert_eq!(networks_from_proc(ROUTE, arp_without_gateway), []);
        assert_eq!(networks_from_proc("", ARP), []);
    }

    #[test]
    fn from_hex_rejects_garbage() {
        assert_eq!(NetworkId::from_hex("abc"), None);
        assert_eq!(NetworkId::from_hex(&"A".repeat(64)), None);
    }
}
