//! Agent-wide access control: failed-login lockout per address, a cap on
//! concurrent connections, and what happens when a viewer asks for
//! exclusive (non-shared) access.
//!
//! Time is injected as milliseconds from any monotonic clock, so the policy
//! is deterministic and testable.

use std::collections::HashMap;
use std::net::IpAddr;

/// What to do when a viewer's `ClientInit` asks for exclusive access
/// (shared flag clear) while others are connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SharePolicy {
    /// Refuse the newcomer and keep the existing viewers (default). A remote
    /// viewer cannot kick out someone already working on the desktop.
    #[default]
    RefuseExclusive,
    /// Disconnect the existing viewers (the classic RFB meaning of a
    /// non-shared `ClientInit`).
    DisconnectOthers,
    /// Ignore the flag: everyone shares.
    AlwaysShare,
}

/// Tunables for [`AccessControl`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessPolicy {
    /// Failed logins allowed from one address before lockouts start.
    pub free_attempts: u32,
    /// Lockout after the first failure past the free ones; it doubles with
    /// every further failure.
    pub base_lockout_ms: u64,
    /// Longest lockout.
    pub max_lockout_ms: u64,
    /// Failures are forgotten after this long without another one.
    pub forget_after_ms: u64,
    /// Most connections at once, in any state (handshaking included).
    pub max_connections: usize,
    /// Most addresses whose failures are remembered (oldest are evicted).
    pub max_tracked_addresses: usize,
    /// Exclusive-access behaviour.
    pub share: SharePolicy,
}

impl AccessPolicy {
    /// 3 free attempts, then 2 s, 4 s, 8 s … capped at 10 minutes; failures
    /// forgotten after an hour; 16 connections; exclusive requests refused
    /// while others are connected.
    pub const DEFAULT: Self = Self {
        free_attempts: 3,
        base_lockout_ms: 2_000,
        max_lockout_ms: 600_000,
        forget_after_ms: 3_600_000,
        max_connections: 16,
        max_tracked_addresses: 4096,
        share: SharePolicy::RefuseExclusive,
    };
}

impl Default for AccessPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Identifies one connection to [`AccessControl`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(u64);

/// Why a connection was refused before the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Too many failed logins from this address; try again after this long.
    LockedOut {
        /// Milliseconds until the lockout ends.
        retry_after_ms: u64,
    },
    /// The connection limit is reached.
    TooManyConnections,
}

/// The outcome of a viewer's `ClientInit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Go ahead; first disconnect these connections (exclusive access).
    Admit {
        /// Connections to close.
        disconnect: Vec<ConnectionId>,
    },
    /// Refuse: another viewer is connected and exclusive access is not
    /// allowed to displace it.
    Refuse,
}

#[derive(Debug, Clone, Copy)]
struct Failures {
    count: u32,
    last_ms: u64,
    locked_until_ms: u64,
}

#[derive(Debug, Clone, Copy)]
struct Connection {
    admitted: bool,
}

/// Agent-wide connection bookkeeping.
#[derive(Debug, Clone, Default)]
pub struct AccessControl {
    policy: AccessPolicy,
    failures: HashMap<IpAddr, Failures>,
    connections: HashMap<ConnectionId, Connection>,
    next_id: u64,
}

impl AccessControl {
    /// Creates the controller.
    #[must_use]
    pub fn new(policy: AccessPolicy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    /// The policy in force.
    #[must_use]
    pub fn policy(&self) -> &AccessPolicy {
        &self.policy
    }

    /// Connections currently counted.
    #[must_use]
    pub fn connections(&self) -> usize {
        self.connections.len()
    }

    /// A new TCP connection from `ip` at `now_ms`: register it, or refuse it
    /// before any protocol bytes are exchanged.
    ///
    /// # Errors
    /// [`Refusal`] if `ip` is locked out or the connection limit is reached.
    pub fn connect(&mut self, ip: IpAddr, now_ms: u64) -> Result<ConnectionId, Refusal> {
        if let Some(retry_after_ms) = self.lockout_remaining(ip, now_ms) {
            return Err(Refusal::LockedOut { retry_after_ms });
        }
        if self.connections.len() >= self.policy.max_connections {
            return Err(Refusal::TooManyConnections);
        }
        self.next_id += 1;
        let id = ConnectionId(self.next_id);
        self.connections.insert(id, Connection { admitted: false });
        Ok(id)
    }

    /// How long `ip` stays locked out, if it is.
    #[must_use]
    pub fn lockout_remaining(&self, ip: IpAddr, now_ms: u64) -> Option<u64> {
        let f = self.failures.get(&ip)?;
        (f.locked_until_ms > now_ms).then(|| f.locked_until_ms - now_ms)
    }

    /// Records a failed login from `ip`.
    pub fn auth_failed(&mut self, ip: IpAddr, now_ms: u64) {
        self.forget_stale(now_ms);
        if !self.failures.contains_key(&ip)
            && self.failures.len() >= self.policy.max_tracked_addresses
            && let Some(oldest) = self
                .failures
                .iter()
                .min_by_key(|(_, f)| f.last_ms)
                .map(|(&ip, _)| ip)
        {
            self.failures.remove(&oldest);
        }
        let entry = self.failures.entry(ip).or_insert(Failures {
            count: 0,
            last_ms: now_ms,
            locked_until_ms: 0,
        });
        entry.count = entry.count.saturating_add(1);
        entry.last_ms = now_ms;
        if let Some(over) = entry.count.checked_sub(self.policy.free_attempts)
            && over > 0
        {
            let factor = 1u64.checked_shl(over - 1).unwrap_or(u64::MAX);
            let lockout = self
                .policy
                .base_lockout_ms
                .saturating_mul(factor)
                .min(self.policy.max_lockout_ms);
            entry.locked_until_ms = now_ms.saturating_add(lockout);
            tracing::warn!(%ip, failures = entry.count, lockout_ms = lockout, "address locked out after failed logins");
        }
    }

    /// Records a successful login from `ip`, clearing its failures.
    pub fn auth_succeeded(&mut self, ip: IpAddr) {
        self.failures.remove(&ip);
    }

    /// Decides a `ClientInit` for connection `id` with the viewer's
    /// `shared` flag. Call [`crate::ServerSession::admit`] on
    /// [`Admission::Admit`] after closing the listed connections (and
    /// reporting them with [`Self::disconnected`]).
    pub fn admit(&mut self, id: ConnectionId, shared: bool) -> Admission {
        let others: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter(|&(&other, c)| other != id && c.admitted)
            .map(|(&other, _)| other)
            .collect();
        let exclusive = !shared && self.policy.share != SharePolicy::AlwaysShare;
        let disconnect = match (exclusive, others.is_empty(), self.policy.share) {
            (false, _, _) | (true, true, _) => Vec::new(),
            (true, false, SharePolicy::DisconnectOthers) => others,
            (true, false, _) => return Admission::Refuse,
        };
        if let Some(c) = self.connections.get_mut(&id) {
            c.admitted = true;
        }
        Admission::Admit { disconnect }
    }

    /// The connection closed (for any reason).
    pub fn disconnected(&mut self, id: ConnectionId) {
        self.connections.remove(&id);
    }

    fn forget_stale(&mut self, now_ms: u64) {
        let forget = self.policy.forget_after_ms;
        self.failures
            .retain(|_, f| f.locked_until_ms > now_ms || now_ms.saturating_sub(f.last_ms) < forget);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    const OTHER: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));

    #[test]
    fn lockout_doubles_after_the_free_attempts_and_is_capped() {
        let mut a = AccessControl::new(AccessPolicy::DEFAULT);
        for _ in 0..3 {
            a.auth_failed(IP, 0);
            assert_eq!(a.lockout_remaining(IP, 0), None);
        }
        let expected = [2_000, 4_000, 8_000, 16_000];
        for (n, want) in expected.into_iter().enumerate() {
            let now = u64::try_from(n).unwrap() * 1_000_000;
            a.auth_failed(IP, now);
            assert_eq!(a.lockout_remaining(IP, now), Some(want));
            assert_eq!(
                a.connect(IP, now + 1),
                Err(Refusal::LockedOut {
                    retry_after_ms: want - 1
                })
            );
            assert!(a.connect(IP, now + want).is_ok());
        }
        for _ in 0..64 {
            a.auth_failed(IP, 10_000_000);
        }
        assert_eq!(a.lockout_remaining(IP, 10_000_000), Some(600_000));
        // Other addresses are unaffected; success clears the record.
        assert!(a.connect(OTHER, 10_000_000).is_ok());
        a.auth_succeeded(IP);
        assert_eq!(a.lockout_remaining(IP, 10_000_000), None);
    }

    #[test]
    fn failures_are_forgotten_after_a_quiet_hour() {
        let mut a = AccessControl::new(AccessPolicy::DEFAULT);
        for _ in 0..3 {
            a.auth_failed(IP, 0);
        }
        a.auth_failed(IP, 3_600_000);
        assert_eq!(a.lockout_remaining(IP, 3_600_000), None);
    }

    #[test]
    fn tracked_addresses_are_bounded() {
        let mut a = AccessControl::new(AccessPolicy {
            max_tracked_addresses: 2,
            ..AccessPolicy::DEFAULT
        });
        for i in 0..10u8 {
            a.auth_failed(IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)), u64::from(i));
        }
        assert_eq!(a.failures.len(), 2);
    }

    #[test]
    fn connection_limit() {
        let mut a = AccessControl::new(AccessPolicy {
            max_connections: 2,
            ..AccessPolicy::DEFAULT
        });
        let first = a.connect(IP, 0).unwrap();
        a.connect(IP, 0).unwrap();
        assert_eq!(a.connect(OTHER, 0), Err(Refusal::TooManyConnections));
        a.disconnected(first);
        assert!(a.connect(OTHER, 0).is_ok());
    }

    #[test]
    fn exclusive_requests_follow_the_share_policy() {
        let mut a = AccessControl::new(AccessPolicy::DEFAULT);
        let first = a.connect(IP, 0).unwrap();
        assert_eq!(
            a.admit(first, false),
            Admission::Admit { disconnect: vec![] }
        );
        let second = a.connect(OTHER, 0).unwrap();
        assert_eq!(a.admit(second, false), Admission::Refuse);
        assert_eq!(
            a.admit(second, true),
            Admission::Admit { disconnect: vec![] }
        );

        let mut kick = AccessControl::new(AccessPolicy {
            share: SharePolicy::DisconnectOthers,
            ..AccessPolicy::DEFAULT
        });
        let first = kick.connect(IP, 0).unwrap();
        kick.admit(first, true);
        let second = kick.connect(OTHER, 0).unwrap();
        assert_eq!(
            kick.admit(second, false),
            Admission::Admit {
                disconnect: vec![first]
            }
        );

        let mut share = AccessControl::new(AccessPolicy {
            share: SharePolicy::AlwaysShare,
            ..AccessPolicy::DEFAULT
        });
        let first = share.connect(IP, 0).unwrap();
        share.admit(first, true);
        let second = share.connect(OTHER, 0).unwrap();
        assert_eq!(
            share.admit(second, false),
            Admission::Admit { disconnect: vec![] }
        );
    }
}
