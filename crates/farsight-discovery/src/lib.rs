//! LAN discovery: mDNS/DNS-SD (`_rfb._tcp`) browsing and advertising per
//! ADR-0010, and opt-in active RFB probing on user-marked networks per
//! ADR-0012. Discovered servers are unverified until their identity is confirmed.
//!
//! Layer: **Adapters** (ADR-0002).
//!
//! Roadmap: M3.5 (browse, probe), M6 (agent advertising).
