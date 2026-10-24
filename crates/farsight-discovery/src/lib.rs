//! LAN discovery: mDNS/DNS-SD (`_rfb._tcp`) browsing and advertising per
//! ADR-0010, and opt-in active RFB probing on user-marked networks per
//! ADR-0012. Discovered servers are unverified until their identity is confirmed.
//!
//! Layer: **Adapters** (ADR-0002).
//!
//! Roadmap: M3.5 (browse, probe), M6 (agent advertising).
//!
//! - [`Browser`] lists `_rfb._tcp` servers from any vendor; [`advertise`]
//!   publishes a Farsight agent.
//! - [`probe()`] actively probes a [`ProbeScope`]. The caller must only start
//!   it on a network the user marked as theirs ([`current_networks`]).
//! - Every result is a [`DiscoveredServer`] with a [`Provenance`] and is
//!   [`Verification::Unverified`]. Names are cleaned with [`sanitize_name`].

pub mod mdns;
pub mod model;
pub mod network;
pub mod probe;
pub mod procfs;
pub mod sanitize;
pub mod scope;
pub mod txt;

pub use mdns::{
    AdvertiseConfig, Advertisement, BrowseEvent, Browser, DiscoveryError, SERVICE_TYPE, advertise,
};
pub use model::{DiscoveredServer, Provenance, RfbInfo, ServerKey, Verification};
pub use network::{NetworkId, current_networks};
pub use probe::{
    HandshakeStep, ProbeConfig, ProbeEvent, ProbeHandshake, ProbeOutcome, ProbeProgress,
    SilentHostPolicy, probe, probe_one,
};
pub use sanitize::sanitize_name;
pub use scope::{InterfaceAddr, ProbeScope, ScopeHost};
pub use txt::{TxtInfo, build_txt, parse_txt};
