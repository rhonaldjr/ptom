//! Security-type negotiation policy and, later, the authentication schemes
//! (VncAuth, RA2, VeNCrypt) as sans-I/O state machines.
//!
//! Layer: **Use cases** (ADR-0002). Depends only on `farsight-rfb`. Cryptographic
//! primitives come from RustCrypto/rustls per ADR-0007.

pub mod vnc_auth;

use farsight_rfb::SecurityType;

/// Security types in order of preference, strongest first.
///
/// `Ra2ne`/`Rane256` are absent because they leave the session unencrypted,
/// and `Tight` is deferred (roadmap Horizon).
const PREFERENCE: [SecurityType; 5] = [
    SecurityType::VeNCrypt,
    SecurityType::Ra256,
    SecurityType::Ra2,
    SecurityType::VncAuth,
    SecurityType::None,
];

/// Which security types an endpoint is willing to use.
///
/// The default is the secure network policy: legacy types are refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SecurityPolicy {
    /// Permit `None` and `VncAuth` (ADR-0007: loopback only unless opted in).
    pub allow_legacy: bool,
}

impl SecurityPolicy {
    /// Policy for connections that never leave the host.
    pub const LOOPBACK: Self = Self { allow_legacy: true };

    /// Policy for connections over a network.
    pub const NETWORK: Self = Self {
        allow_legacy: false,
    };

    /// Chooses the strongest security type that the peer offered and this
    /// policy permits, or `None` if there is no acceptable overlap.
    #[must_use]
    pub fn select(self, offered: &[SecurityType]) -> Option<SecurityType> {
        PREFERENCE
            .into_iter()
            .filter(|candidate| self.allow_legacy || !candidate.is_legacy_insecure())
            .find(|candidate| offered.contains(candidate))
    }

    /// Filters the agent's configured security types down to those this
    /// policy permits, keeping the configured order.
    #[must_use]
    pub fn server_offer(self, configured: &[SecurityType]) -> Vec<SecurityType> {
        configured
            .iter()
            .copied()
            .filter(|t| self.allow_legacy || !t.is_legacy_insecure())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use farsight_rfb::SecurityType as T;
    use proptest::prelude::*;

    #[test]
    fn prefers_strongest_type_regardless_of_offer_order() {
        let offered = [T::VncAuth, T::Ra2, T::VeNCrypt];
        assert_eq!(SecurityPolicy::NETWORK.select(&offered), Some(T::VeNCrypt));
    }

    #[test]
    fn network_policy_refuses_legacy_only_servers() {
        assert_eq!(SecurityPolicy::NETWORK.select(&[T::VncAuth, T::None]), None);
    }

    #[test]
    fn loopback_policy_accepts_legacy_types() {
        assert_eq!(
            SecurityPolicy::LOOPBACK.select(&[T::None, T::VncAuth]),
            Some(T::VncAuth)
        );
    }

    #[test]
    fn default_policy_is_the_network_policy() {
        assert_eq!(SecurityPolicy::default(), SecurityPolicy::NETWORK);
    }

    #[test]
    fn ignores_unencrypted_and_unknown_types() {
        assert_eq!(
            SecurityPolicy::NETWORK.select(&[T::Ra2ne, T::Rane256, T::Unknown(42)]),
            None
        );
    }

    #[test]
    fn network_offer_drops_legacy_types() {
        let configured = [T::VeNCrypt, T::VncAuth, T::None];
        assert_eq!(
            SecurityPolicy::NETWORK.server_offer(&configured),
            [T::VeNCrypt]
        );
        assert_eq!(
            SecurityPolicy::LOOPBACK.server_offer(&configured),
            configured
        );
    }

    proptest! {
        #[test]
        fn network_policy_never_selects_legacy(codes in proptest::collection::vec(any::<u8>(), 0..16)) {
            let offered: Vec<_> = codes.into_iter().map(T::from_code).collect();
            if let Some(chosen) = SecurityPolicy::NETWORK.select(&offered) {
                prop_assert!(!chosen.is_legacy_insecure());
                prop_assert!(offered.contains(&chosen));
            }
        }
    }
}
