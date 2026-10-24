//! Which hosts the active prober may contact (ADR-0012).
//!
//! The scope is limited to directly attached private (RFC 1918) and
//! link-local IPv4 networks, plus hosts in the neighbour (ARP) table. A
//! network larger than /22 is narrowed to the /24 containing our address.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

use crate::procfs::parse_arp;

/// The widest attached network that is probed whole.
pub const WIDEST_PROBED_PREFIX: u8 = 22;
/// The prefix probed instead when an attached network is wider than
/// [`WIDEST_PROBED_PREFIX`].
pub const NARROWED_PREFIX: u8 = 24;

/// An IPv4 address assigned to one of this machine's interfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceAddr {
    /// Our address.
    pub addr: Ipv4Addr,
    /// The attached network's prefix length.
    pub prefix_len: u8,
}

/// One host the prober may contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeHost {
    /// The host's address.
    pub addr: IpAddr,
    /// The host is known to exist (it is in the neighbour table or was named
    /// explicitly), so every port is probed even if the first one is silent.
    pub known_alive: bool,
}

/// The set of hosts an active probe may contact. Build it with
/// [`ProbeScope::compute`] / [`ProbeScope::from_local_networks`], or
/// [`ProbeScope::from_hosts`] for an explicit list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeScope {
    hosts: Vec<ScopeHost>,
}

impl ProbeScope {
    /// Computes the scope from this machine's interface addresses and its
    /// neighbour table. Only private and link-local IPv4 networks are used;
    /// our own addresses, loopback, and network/broadcast addresses are
    /// excluded.
    #[must_use]
    pub fn compute(interfaces: &[InterfaceAddr], neighbours: &[Ipv4Addr]) -> Self {
        let own: Vec<Ipv4Addr> = interfaces.iter().map(|i| i.addr).collect();
        let subnets: Vec<Subnet> = interfaces
            .iter()
            .filter(|i| is_local_scope(i.addr))
            .filter_map(|i| Subnet::probed(i.addr, i.prefix_len))
            .collect();
        let excluded = |addr: Ipv4Addr| {
            own.contains(&addr)
                || !is_local_scope(addr)
                || subnets.iter().any(|s| s.is_reserved(addr))
        };
        let mut hosts = BTreeMap::new();
        for subnet in &subnets {
            for addr in subnet.hosts().filter(|a| !excluded(*a)) {
                hosts.entry(addr).or_insert(false);
            }
        }
        for &addr in neighbours.iter().filter(|a| !excluded(**a)) {
            hosts.insert(addr, true);
        }
        Self {
            hosts: hosts
                .into_iter()
                .map(|(addr, known_alive)| ScopeHost {
                    addr: IpAddr::V4(addr),
                    known_alive,
                })
                .collect(),
        }
    }

    /// Computes the scope for this machine's current interfaces and (on
    /// Linux) the `/proc/net/arp` neighbour table.
    ///
    /// # Errors
    /// The interface list could not be read.
    pub fn from_local_networks() -> std::io::Result<Self> {
        let interfaces: Vec<InterfaceAddr> = if_addrs::get_if_addrs()?
            .into_iter()
            .filter_map(|iface| match iface.addr {
                if_addrs::IfAddr::V4(v4) => Some(InterfaceAddr {
                    addr: v4.ip,
                    prefix_len: v4.prefixlen,
                }),
                if_addrs::IfAddr::V6(_) => None,
            })
            .collect();
        let neighbours: Vec<Ipv4Addr> = if cfg!(target_os = "linux") {
            parse_arp(&std::fs::read_to_string("/proc/net/arp").unwrap_or_default())
                .into_iter()
                .map(|entry| entry.ip)
                .collect()
        } else {
            Vec::new()
        };
        Ok(Self::compute(&interfaces, &neighbours))
    }

    /// A scope of exactly these hosts, each probed on every port. No
    /// filtering is applied: the caller is responsible for keeping it local.
    #[must_use]
    pub fn from_hosts(hosts: impl IntoIterator<Item = IpAddr>) -> Self {
        let mut hosts: Vec<IpAddr> = hosts.into_iter().collect();
        hosts.sort_unstable();
        hosts.dedup();
        Self {
            hosts: hosts
                .into_iter()
                .map(|addr| ScopeHost {
                    addr,
                    known_alive: true,
                })
                .collect(),
        }
    }

    /// The hosts in scope, in address order.
    #[must_use]
    pub fn hosts(&self) -> &[ScopeHost] {
        &self.hosts
    }

    /// Number of hosts in scope.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    /// Whether no host is in scope.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }
}

/// Private (RFC 1918) or link-local (169.254/16), and not loopback.
fn is_local_scope(addr: Ipv4Addr) -> bool {
    (addr.is_private() || addr.is_link_local()) && !addr.is_loopback()
}

#[derive(Debug, Clone, Copy)]
struct Subnet {
    network: u32,
    prefix: u8,
}

impl Subnet {
    /// The part of `addr`'s attached network that is probed, after narrowing.
    fn probed(addr: Ipv4Addr, prefix: u8) -> Option<Self> {
        if prefix > 32 {
            return None;
        }
        let prefix = if prefix < WIDEST_PROBED_PREFIX {
            NARROWED_PREFIX
        } else {
            prefix
        };
        Some(Self {
            network: u32::from(addr) & mask(prefix),
            prefix,
        })
    }

    fn broadcast(self) -> u32 {
        self.network | !mask(self.prefix)
    }

    /// Network and broadcast addresses (none for /31 and /32, RFC 3021).
    fn is_reserved(self, addr: Ipv4Addr) -> bool {
        let addr = u32::from(addr);
        self.prefix < 31 && (addr == self.network || addr == self.broadcast())
    }

    fn hosts(self) -> impl Iterator<Item = Ipv4Addr> {
        let (first, last) = if self.prefix >= 31 {
            (self.network, self.broadcast())
        } else {
            (self.network + 1, self.broadcast() - 1)
        };
        (first..=last).map(Ipv4Addr::from)
    }
}

fn mask(prefix: u8) -> u32 {
    u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(a: [u8; 4], prefix_len: u8) -> InterfaceAddr {
        InterfaceAddr {
            addr: Ipv4Addr::from(a),
            prefix_len,
        }
    }

    fn v4(a: [u8; 4]) -> IpAddr {
        IpAddr::V4(Ipv4Addr::from(a))
    }

    fn addrs(scope: &ProbeScope) -> Vec<IpAddr> {
        scope.hosts().iter().map(|h| h.addr).collect()
    }

    #[test]
    fn home_slash_24() {
        let scope = ProbeScope::compute(&[iface([192, 168, 1, 10], 24)], &[]);
        assert_eq!(scope.len(), 253);
        let hosts = addrs(&scope);
        assert_eq!(hosts.first(), Some(&v4([192, 168, 1, 1])));
        assert_eq!(hosts.last(), Some(&v4([192, 168, 1, 254])));
        assert!(!hosts.contains(&v4([192, 168, 1, 10])));
        assert!(!hosts.contains(&v4([192, 168, 1, 0])));
        assert!(!hosts.contains(&v4([192, 168, 1, 255])));
        assert!(scope.hosts().iter().all(|h| !h.known_alive));
    }

    #[test]
    fn slash_22_is_probed_whole() {
        let scope = ProbeScope::compute(&[iface([10, 0, 5, 7], 22)], &[]);
        assert_eq!(scope.len(), 1021);
        let hosts = addrs(&scope);
        assert_eq!(hosts.first(), Some(&v4([10, 0, 4, 1])));
        assert_eq!(hosts.last(), Some(&v4([10, 0, 7, 254])));
    }

    #[test]
    fn wider_than_slash_22_is_narrowed_to_our_slash_24() {
        for prefix in [8, 16, 21] {
            let scope = ProbeScope::compute(&[iface([10, 20, 30, 40], prefix)], &[]);
            assert_eq!(scope.len(), 253, "prefix {prefix}");
            let hosts = addrs(&scope);
            assert_eq!(hosts.first(), Some(&v4([10, 20, 30, 1])));
            assert_eq!(hosts.last(), Some(&v4([10, 20, 30, 254])));
        }
    }

    #[test]
    fn public_and_loopback_networks_are_skipped() {
        let scope = ProbeScope::compute(
            &[
                iface([127, 0, 0, 1], 8),
                iface([203, 0, 113, 5], 24),
                iface([100, 64, 0, 9], 24), // CGNAT is not RFC 1918
            ],
            &[Ipv4Addr::new(203, 0, 113, 1), Ipv4Addr::LOCALHOST],
        );
        assert!(scope.is_empty());
    }

    #[test]
    fn link_local_and_tiny_networks() {
        let scope = ProbeScope::compute(&[iface([169, 254, 3, 4], 16)], &[]);
        assert_eq!(scope.len(), 253);
        assert!(addrs(&scope).contains(&v4([169, 254, 3, 1])));

        let p2p = ProbeScope::compute(&[iface([10, 9, 9, 0], 31)], &[]);
        assert_eq!(addrs(&p2p), vec![v4([10, 9, 9, 1])]);

        let host_route = ProbeScope::compute(&[iface([10, 9, 9, 9], 32)], &[]);
        assert!(host_route.is_empty());
    }

    #[test]
    fn neighbours_are_added_and_marked_alive() {
        let scope = ProbeScope::compute(
            &[iface([192, 168, 1, 10], 24), iface([172, 16, 0, 2], 16)],
            &[
                Ipv4Addr::new(192, 168, 1, 1),
                Ipv4Addr::new(172, 16, 9, 9), // outside our narrowed /24
                Ipv4Addr::new(192, 168, 1, 10), // ourselves
                Ipv4Addr::new(192, 168, 1, 255), // broadcast
                Ipv4Addr::new(8, 8, 8, 8),    // not local
            ],
        );
        assert_eq!(scope.len(), 253 + 253 + 1);
        let find = |a| scope.hosts().iter().find(|h| h.addr == v4(a)).copied();
        assert_eq!(find([192, 168, 1, 1]).map(|h| h.known_alive), Some(true));
        assert_eq!(find([172, 16, 9, 9]).map(|h| h.known_alive), Some(true));
        assert_eq!(find([192, 168, 1, 2]).map(|h| h.known_alive), Some(false));
        assert!(find([192, 168, 1, 10]).is_none());
        assert!(find([192, 168, 1, 255]).is_none());
        assert!(find([8, 8, 8, 8]).is_none());
    }

    #[test]
    fn own_addresses_on_other_interfaces_are_excluded() {
        let scope = ProbeScope::compute(
            &[iface([192, 168, 1, 10], 24), iface([192, 168, 1, 11], 24)],
            &[],
        );
        assert_eq!(scope.len(), 252);
    }

    #[test]
    fn explicit_hosts_are_deduplicated() {
        let scope = ProbeScope::from_hosts([v4([127, 0, 0, 1]), v4([127, 0, 0, 1])]);
        assert_eq!(scope.len(), 1);
        assert!(scope.hosts()[0].known_alive);
    }

    #[test]
    fn invalid_prefix_is_ignored() {
        assert!(ProbeScope::compute(&[iface([10, 0, 0, 1], 40)], &[]).is_empty());
    }
}
