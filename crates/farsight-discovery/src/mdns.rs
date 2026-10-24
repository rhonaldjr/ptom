//! mDNS / DNS-SD browsing and advertising of `_rfb._tcp` (ADR-0010).

use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use tracing::debug;

use crate::model::{DiscoveredServer, Provenance, ServerKey, Verification};
use crate::sanitize::sanitize_name;
use crate::txt::{InvalidFingerprint, build_txt, parse_txt};

/// The DNS-SD service type for RFB servers, used by every vendor.
pub const SERVICE_TYPE: &str = "_rfb._tcp.local.";

/// How long dropping an [`Advertisement`] waits for the goodbye to be sent.
const UNREGISTER_WAIT: Duration = Duration::from_millis(250);

/// Why mDNS browsing or advertising failed.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// The mDNS daemon failed.
    #[error("mDNS: {0}")]
    Mdns(#[from] mdns_sd::Error),
    /// The fingerprint to advertise was not 64 hex digits.
    #[error(transparent)]
    Fingerprint(#[from] InvalidFingerprint),
    /// The instance or host name was empty after sanitising.
    #[error("the name to advertise is empty")]
    EmptyName,
}

/// A change in the set of advertised servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseEvent {
    /// A server was found, or its details changed. Replace any earlier entry
    /// with the same key.
    Resolved(DiscoveredServer),
    /// A server stopped advertising.
    Removed(ServerKey),
}

/// A long-running browse of `_rfb._tcp.local.` that lists servers from any
/// vendor. Stops when dropped.
pub struct Browser {
    daemon: ServiceDaemon,
    events: mdns_sd::Receiver<ServiceEvent>,
}

impl Browser {
    /// Starts browsing on all interfaces.
    ///
    /// # Errors
    /// [`DiscoveryError::Mdns`] if the mDNS daemon could not start.
    pub fn start() -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new()?;
        let events = daemon.browse(SERVICE_TYPE)?;
        Ok(Self { daemon, events })
    }

    /// Waits for the next change. Returns `None` once the browse has ended.
    pub async fn next_event(&mut self) -> Option<BrowseEvent> {
        loop {
            match self.events.recv_async().await.ok()? {
                ServiceEvent::ServiceResolved(service) if service.is_valid() => {
                    return Some(BrowseEvent::Resolved(server_from_mdns(&service)));
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    return Some(BrowseEvent::Removed(mdns_key(&fullname)));
                }
                ServiceEvent::SearchStopped(_) => return None,
                _ => {}
            }
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
        let _ = self.daemon.shutdown();
    }
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser").finish_non_exhaustive()
    }
}

fn mdns_key(fullname: &str) -> ServerKey {
    ServerKey::new(format!("mdns:{fullname}"))
}

fn server_from_mdns(service: &ResolvedService) -> DiscoveredServer {
    let mut addresses: Vec<IpAddr> = service
        .addresses
        .iter()
        .map(mdns_sd::ScopedIp::to_ip_addr)
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    let host_name = Some(sanitize_name(&service.host)).filter(|h| !h.is_empty());
    let name = Some(instance_name(&service.fullname, SERVICE_TYPE))
        .filter(|n| !n.is_empty())
        .or_else(|| host_name.clone())
        .or_else(|| addresses.first().map(ToString::to_string))
        .unwrap_or_default();
    let txt = parse_txt(service.txt_properties.iter().map(|p| (p.key(), p.val())));
    DiscoveredServer {
        key: mdns_key(&service.fullname),
        name,
        host_name,
        addresses,
        port: service.port,
        provenance: Provenance::Mdns,
        verification: Verification::Unverified,
        txt,
        rfb: None,
    }
}

/// The display name of a service instance: `fullname` without the service
/// type, with DNS escapes (`\.`, `\\`, `\DDD`) undone, then sanitised.
#[must_use]
pub fn instance_name(fullname: &str, service_type: &str) -> String {
    let instance = fullname
        .strip_suffix(service_type)
        .and_then(|rest| rest.strip_suffix('.'))
        .unwrap_or(fullname);
    sanitize_name(&unescape_label(instance))
}

fn unescape_label(label: &str) -> String {
    let mut bytes = Vec::with_capacity(label.len());
    let mut rest = label.as_bytes();
    while let Some((&first, tail)) = rest.split_first() {
        rest = tail;
        if first != b'\\' {
            bytes.push(first);
            continue;
        }
        match rest {
            [a, b, c, tail @ ..] if [a, b, c].iter().all(|d| d.is_ascii_digit()) => {
                let value =
                    u16::from(a - b'0') * 100 + u16::from(b - b'0') * 10 + u16::from(c - b'0');
                bytes.push(u8::try_from(value).unwrap_or(b'?'));
                rest = tail;
            }
            [escaped, tail @ ..] => {
                bytes.push(*escaped);
                rest = tail;
            }
            [] => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// What an agent advertises (ADR-0010). Only presence, the port and the
/// identity fingerprint are published: no usernames, OS or desktop names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertiseConfig {
    /// Instance name shown to viewers (defaults to the host name in the agent).
    pub instance_name: String,
    /// mDNS host name; `.local.` is appended if missing.
    pub host_name: String,
    /// Port the agent listens on.
    pub port: u16,
    /// Hex SHA-256 fingerprint of the agent's TLS/RA2 identity key.
    pub fingerprint: String,
    /// Addresses to publish; empty publishes every interface address and
    /// follows changes.
    pub addresses: Vec<IpAddr>,
}

/// A registered advertisement. Dropping it unregisters the instance (sending
/// an mDNS goodbye) and stops its daemon.
pub struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertisement {
    /// The registered instance's full DNS-SD name.
    #[must_use]
    pub fn fullname(&self) -> &str {
        &self.fullname
    }
}

impl std::fmt::Debug for Advertisement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Advertisement")
            .field("fullname", &self.fullname)
            .finish_non_exhaustive()
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        if let Ok(status) = self.daemon.unregister(&self.fullname) {
            let _ = status.recv_timeout(UNREGISTER_WAIT);
        }
        let _ = self.daemon.shutdown();
    }
}

/// Registers an `_rfb._tcp` instance with the minimal TXT record
/// `farsight=1`, `v=3.8`, `fp=<hex>`.
///
/// # Errors
/// [`DiscoveryError`] if a name is empty, the fingerprint is invalid, or the
/// mDNS daemon fails.
pub fn advertise(config: &AdvertiseConfig) -> Result<Advertisement, DiscoveryError> {
    let instance = sanitize_name(&config.instance_name);
    let host = host_label(&config.host_name).ok_or(DiscoveryError::EmptyName)?;
    if instance.is_empty() {
        return Err(DiscoveryError::EmptyName);
    }
    let txt = build_txt(&config.fingerprint)?;
    let addresses: &[IpAddr] = &config.addresses;
    let mut info = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &host,
        addresses,
        config.port,
        &txt[..],
    )?;
    if config.addresses.is_empty() {
        info = info.enable_addr_auto();
    }
    let fullname = info.get_fullname().to_owned();
    let daemon = ServiceDaemon::new()?;
    daemon.register(info)?;
    debug!(port = config.port, "advertising _rfb._tcp");
    Ok(Advertisement { daemon, fullname })
}

/// A host name usable in mDNS: sanitised, spaces replaced, ending `.local.`.
fn host_label(raw: &str) -> Option<String> {
    let clean = sanitize_name(raw).replace(' ', "-");
    let base = clean
        .strip_suffix(".local.")
        .or_else(|| clean.strip_suffix(".local"))
        .unwrap_or(&clean)
        .trim_end_matches('.');
    (!base.is_empty()).then(|| format!("{base}.local."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_names_are_unescaped_and_sanitised() {
        assert_eq!(
            instance_name("Living Room._rfb._tcp.local.", SERVICE_TYPE),
            "Living Room"
        );
        assert_eq!(
            instance_name(r"my\.box\\x._rfb._tcp.local.", SERVICE_TYPE),
            r"my.box\x"
        );
        assert_eq!(
            instance_name(r"caf\195\169._rfb._tcp.local.", SERVICE_TYPE),
            "café"
        );
        assert_eq!(
            instance_name("evil\u{202E}txt._rfb._tcp.local.", SERVICE_TYPE),
            "eviltxt"
        );
        assert_eq!(instance_name(r"bad\999\", SERVICE_TYPE), "bad?");
        assert_eq!(instance_name("", SERVICE_TYPE), "");
    }

    #[test]
    fn host_labels() {
        assert_eq!(host_label("desk").as_deref(), Some("desk.local."));
        assert_eq!(host_label("desk.local").as_deref(), Some("desk.local."));
        assert_eq!(host_label("desk.local.").as_deref(), Some("desk.local."));
        assert_eq!(host_label("my desk").as_deref(), Some("my-desk.local."));
        assert_eq!(host_label("\u{202E}"), None);
    }

    #[test]
    fn advertise_validates_before_starting_a_daemon() {
        let config = AdvertiseConfig {
            instance_name: "desk".into(),
            host_name: "desk".into(),
            port: 5900,
            fingerprint: "nothex".into(),
            addresses: Vec::new(),
        };
        assert!(matches!(
            advertise(&config),
            Err(DiscoveryError::Fingerprint(_))
        ));
        let empty = AdvertiseConfig {
            instance_name: "\u{200F}".into(),
            ..config
        };
        assert!(matches!(advertise(&empty), Err(DiscoveryError::EmptyName)));
    }
}
