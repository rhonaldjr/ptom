//! What discovery reports about a server.

use std::net::IpAddr;

use farsight_rfb::{ProtocolVersion, SecurityType};

use crate::txt::TxtInfo;

/// How a server was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provenance {
    /// It advertised `_rfb._tcp` over mDNS / DNS-SD.
    Mdns,
    /// The active prober found it (ADR-0012).
    Probe,
}

/// Whether a server's identity has been confirmed. Discovery never confirms
/// identity: that happens on connect (TOFU or a pinned fingerprint, ADR-0017).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verification {
    /// Nothing about this entry has been verified. Names, TXT fields and
    /// probe results are all claims made by whoever answered.
    Unverified,
}

/// An opaque, stable key for a discovered entry, used to match removals to
/// earlier additions. It may contain raw peer-supplied text: never display it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServerKey(String);

impl ServerKey {
    pub(crate) fn new(key: String) -> Self {
        Self(key)
    }
}

/// What a probe learned from a server's RFB handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RfbInfo {
    /// The protocol version used for the handshake (the server's, capped at 3.8).
    pub version: ProtocolVersion,
    /// The security types offered. Empty if the server refused the connection
    /// (for example after too many failed logins).
    pub security_types: Vec<SecurityType>,
    /// The server offers an encrypting security type (VeNCrypt, RA2 or
    /// RA2_256). Advisory only: it can be spoofed, and a man in the middle can
    /// strip it.
    pub offers_encryption: bool,
}

impl RfbInfo {
    /// Builds the info, dropping repeated types (keeping the server's order)
    /// and deriving [`RfbInfo::offers_encryption`].
    #[must_use]
    pub fn new(version: ProtocolVersion, offered: Vec<SecurityType>) -> Self {
        // Some servers list a type more than once (TigerVNC repeats VeNCrypt).
        let mut security_types = Vec::with_capacity(offered.len());
        for security in offered {
            if !security_types.contains(&security) {
                security_types.push(security);
            }
        }
        let offers_encryption = security_types.iter().any(|t| is_encrypting(*t));
        Self {
            version,
            security_types,
            offers_encryption,
        }
    }
}

/// Whether a security type encrypts the session.
#[must_use]
pub fn is_encrypting(security: SecurityType) -> bool {
    matches!(
        security,
        SecurityType::VeNCrypt | SecurityType::Ra2 | SecurityType::Ra256
    )
}

/// A server found on the local network. Always [`Verification::Unverified`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredServer {
    /// Opaque key for matching later updates and removals.
    pub key: ServerKey,
    /// Display name, already sanitised (`sanitize_name`). For probed servers
    /// this is the address.
    pub name: String,
    /// Host name, sanitised, when mDNS supplied one (for example `nas.local.`).
    pub host_name: Option<String>,
    /// Addresses the server is reachable at.
    pub addresses: Vec<IpAddr>,
    /// TCP port.
    pub port: u16,
    /// How it was found.
    pub provenance: Provenance,
    /// Always `Unverified` here.
    pub verification: Verification,
    /// Parsed TXT record (mDNS only; default for probed servers).
    pub txt: TxtInfo,
    /// Handshake details (probed servers only).
    pub rfb: Option<RfbInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_is_derived_from_types() {
        let plain = RfbInfo::new(
            ProtocolVersion::V3_8,
            vec![
                SecurityType::VncAuth,
                SecurityType::None,
                SecurityType::Ra2ne,
            ],
        );
        assert!(!plain.offers_encryption);
        let repeated = RfbInfo::new(
            ProtocolVersion::V3_8,
            vec![
                SecurityType::VeNCrypt,
                SecurityType::VeNCrypt,
                SecurityType::VncAuth,
            ],
        );
        assert_eq!(
            repeated.security_types,
            vec![SecurityType::VeNCrypt, SecurityType::VncAuth]
        );
        for t in [
            SecurityType::VeNCrypt,
            SecurityType::Ra2,
            SecurityType::Ra256,
        ] {
            assert!(
                RfbInfo::new(ProtocolVersion::V3_8, vec![SecurityType::VncAuth, t])
                    .offers_encryption
            );
        }
    }
}
