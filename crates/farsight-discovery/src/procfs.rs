//! Pure parsers for Linux `/proc/net/route` and `/proc/net/arp`.
//!
//! The functions take file contents as text so tests can feed fixtures. Lines
//! that don't parse are skipped.

use std::net::Ipv4Addr;

/// `RTF_UP`: the route is usable.
const RTF_UP: u32 = 0x1;
/// `RTF_GATEWAY`: the destination is reached through a gateway.
const RTF_GATEWAY: u32 = 0x2;
/// `ATF_COM`: the neighbour entry is complete (the MAC is known).
const ATF_COM: u32 = 0x2;

/// One IPv4 route from `/proc/net/route`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Interface name.
    pub interface: String,
    /// Destination network.
    pub destination: Ipv4Addr,
    /// Gateway, or `0.0.0.0` for on-link routes.
    pub gateway: Ipv4Addr,
    /// Route flags (`RTF_*`).
    pub flags: u32,
    /// Destination netmask.
    pub mask: Ipv4Addr,
}

impl Route {
    /// Whether this is a usable default route through a gateway.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.destination.is_unspecified()
            && self.mask.is_unspecified()
            && self.flags & (RTF_UP | RTF_GATEWAY) == RTF_UP | RTF_GATEWAY
    }

    /// Whether this is a usable on-link (directly attached) route.
    #[must_use]
    pub fn is_on_link(&self) -> bool {
        self.flags & RTF_UP != 0 && self.flags & RTF_GATEWAY == 0 && self.gateway.is_unspecified()
    }

    /// Whether `addr` lies in this route's destination network.
    #[must_use]
    pub fn contains(&self, addr: Ipv4Addr) -> bool {
        let mask = u32::from(self.mask);
        u32::from(addr) & mask == u32::from(self.destination) & mask
    }
}

/// One neighbour from `/proc/net/arp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArpEntry {
    /// Neighbour address.
    pub ip: Ipv4Addr,
    /// Hardware address, lowercase `aa:bb:cc:dd:ee:ff`.
    pub mac: String,
    /// Interface name.
    pub interface: String,
}

/// Parses `/proc/net/route`. Addresses there are the kernel's network-order
/// words printed as native-endian hex, so this decodes them with the host's
/// byte order (it describes the machine it runs on).
#[must_use]
pub fn parse_routes(text: &str) -> Vec<Route> {
    text.lines().skip(1).filter_map(parse_route_line).collect()
}

fn parse_route_line(line: &str) -> Option<Route> {
    let mut fields = line.split_whitespace();
    let interface = fields.next()?.to_owned();
    let destination = proc_ipv4(fields.next()?)?;
    let gateway = proc_ipv4(fields.next()?)?;
    let flags = u32::from_str_radix(fields.next()?, 16).ok()?;
    // RefCnt, Use, Metric.
    let mask = proc_ipv4(fields.nth(3)?)?;
    Some(Route {
        interface,
        destination,
        gateway,
        flags,
        mask,
    })
}

fn proc_ipv4(hex: &str) -> Option<Ipv4Addr> {
    let word = u32::from_str_radix(hex, 16).ok()?;
    Some(Ipv4Addr::from(word.to_ne_bytes()))
}

/// Parses `/proc/net/arp`, keeping only complete entries with a valid MAC.
#[must_use]
pub fn parse_arp(text: &str) -> Vec<ArpEntry> {
    text.lines().skip(1).filter_map(parse_arp_line).collect()
}

fn parse_arp_line(line: &str) -> Option<ArpEntry> {
    let mut fields = line.split_whitespace();
    let ip = fields.next()?.parse().ok()?;
    let _hw_type = fields.next()?;
    let flags = u32::from_str_radix(fields.next()?.trim_start_matches("0x"), 16).ok()?;
    let mac = normalise_mac(fields.next()?)?;
    let _mask = fields.next()?;
    let interface = fields.next()?.to_owned();
    (flags & ATF_COM != 0).then_some(ArpEntry { ip, mac, interface })
}

fn normalise_mac(raw: &str) -> Option<String> {
    let octets: Vec<&str> = raw.split(':').collect();
    let valid = octets.len() == 6
        && octets
            .iter()
            .all(|o| o.len() == 2 && o.bytes().all(|b| b.is_ascii_hexdigit()));
    let mac = raw.to_ascii_lowercase();
    (valid && mac != "00:00:00:00:00:00").then_some(mac)
}

/// Prefix length of a contiguous netmask, or `None` if it isn't contiguous.
#[must_use]
pub fn prefix_len(mask: Ipv4Addr) -> Option<u8> {
    let bits = u32::from(mask);
    let ones = bits.leading_ones();
    if bits.checked_shl(ones).unwrap_or(0) != 0 {
        return None;
    }
    u8::try_from(ones).ok()
}

/// Fixtures shared with other modules' tests.
#[cfg(test)]
pub(crate) mod fixtures {
    /// A laptop on Wi-Fi (192.168.1.0/24, gateway .1) with a docker bridge,
    /// as a little-endian kernel prints it.
    pub const ROUTE: &str = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlp2s0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
wlp2s0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0
docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0
garbage line
";

    pub const ARP: &str = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.1.1      0x1         0x2         AA:BB:CC:DD:EE:01     *        wlp2s0
192.168.1.20     0x1         0x2         aa:bb:cc:dd:ee:14     *        wlp2s0
192.168.1.99     0x1         0x0         00:00:00:00:00:00     *        wlp2s0
192.168.1.30     0x1         0x2         not-a-mac             *        wlp2s0
";
}

#[cfg(test)]
mod tests {
    use super::fixtures::{ARP, ROUTE};
    use super::*;

    #[cfg(target_endian = "little")]
    #[test]
    fn parses_routes() {
        let routes = parse_routes(ROUTE);
        assert_eq!(routes.len(), 3);
        let default = &routes[0];
        assert!(default.is_default());
        assert_eq!(default.gateway, Ipv4Addr::new(192, 168, 1, 1));
        let link = &routes[1];
        assert!(link.is_on_link());
        assert!(!link.is_default());
        assert_eq!(link.destination, Ipv4Addr::new(192, 168, 1, 0));
        assert_eq!(prefix_len(link.mask), Some(24));
        assert!(link.contains(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(routes[2].destination, Ipv4Addr::new(172, 17, 0, 0));
    }

    #[test]
    fn parses_complete_arp_entries_only() {
        let arp = parse_arp(ARP);
        assert_eq!(arp.len(), 2);
        assert_eq!(arp[0].ip, Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(arp[0].mac, "aa:bb:cc:dd:ee:01");
        assert_eq!(arp[0].interface, "wlp2s0");
    }

    #[test]
    fn tolerates_empty_and_hostile_input() {
        assert_eq!(parse_routes(""), []);
        assert_eq!(parse_arp(""), []);
        assert_eq!(parse_routes("h\n\u{0}\u{202e} xx yy zz 1 2 3 4\n"), []);
        assert_eq!(
            parse_arp("h\n1.2.3.4 0x1 0xZZ aa:bb:cc:dd:ee:ff * eth0\n"),
            []
        );
    }

    #[test]
    fn prefix_lengths() {
        assert_eq!(prefix_len(Ipv4Addr::new(255, 255, 255, 0)), Some(24));
        assert_eq!(prefix_len(Ipv4Addr::new(255, 255, 252, 0)), Some(22));
        assert_eq!(prefix_len(Ipv4Addr::UNSPECIFIED), Some(0));
        assert_eq!(prefix_len(Ipv4Addr::BROADCAST), Some(32));
        assert_eq!(prefix_len(Ipv4Addr::new(255, 0, 255, 0)), None);
    }
}
