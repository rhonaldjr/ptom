//! Agent-wide access control: failed-login lockout per address (IPv6 per
//! /64), a global failure budget, caps on connections that have not been let
//! in yet, a pause after "Disconnect everyone", and what happens when a
//! viewer asks for exclusive (non-shared) access.
//!
//! Time is injected as milliseconds from any monotonic clock, so the policy
//! is deterministic and testable.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv6Addr};

/// What happens when a viewer asks for exclusive access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SharePolicy {
    /// Refuse the exclusive request while others are connected (default).
    #[default]
    RefuseExclusive,
    /// Disconnect the others.
    DisconnectOthers,
    /// Ignore the request: everyone shares.
    AlwaysShare,
}

/// Limits and timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessPolicy {
    /// Failed logins per address before lockout starts.
    pub free_attempts: u32,
    /// First lockout; doubles with each further failure.
    pub base_lockout_ms: u64,
    /// Longest lockout.
    pub max_lockout_ms: u64,
    /// Failures are forgotten after this long without new ones.
    pub forget_after_ms: u64,
    /// Most connections at once (admitted or not).
    pub max_connections: usize,
    /// Most connections not yet admitted (handshaking or awaiting approval),
    /// so idle strangers can never take the slots of admitted viewers.
    pub max_pending: usize,
    /// Most not-yet-admitted connections from one address (or IPv6 /64).
    pub max_pending_per_address: usize,
    /// Most addresses with failure records kept.
    pub max_tracked_addresses: usize,
    /// Failed logins from all addresses within `global_window_ms` that
    /// trigger an agent-wide pause (address-rotation guessing).
    pub global_failure_budget: usize,
    /// Window for the global failure budget.
    pub global_window_ms: u64,
    /// Agent-wide pause once the budget is spent.
    pub global_lockout_ms: u64,
    /// Exclusive-access behaviour.
    pub share: SharePolicy,
}

impl AccessPolicy {
    /// Defaults: 3 free attempts, 2 s doubling to 10 min, forgotten after
    /// 1 h; 16 connections, of which at most 8 pending and 2 pending per
    /// address; 30 failures a minute pause the agent for a minute.
    pub const DEFAULT: Self = Self {
        free_attempts: 3,
        base_lockout_ms: 2_000,
        max_lockout_ms: 600_000,
        forget_after_ms: 3_600_000,
        max_connections: 16,
        max_pending: 8,
        max_pending_per_address: 2,
        max_tracked_addresses: 4096,
        global_failure_budget: 30,
        global_window_ms: 60_000,
        global_lockout_ms: 60_000,
        share: SharePolicy::RefuseExclusive,
    };
}

impl Default for AccessPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Identifies one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(u64);

/// Why a connection was refused at accept time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Too many failed logins from this address (or the whole agent).
    LockedOut {
        /// When to try again.
        retry_after_ms: u64,
    },
    /// The connection limit is reached.
    TooManyConnections,
    /// Too many connections from this address are still handshaking.
    TooManyPending,
    /// Sharing is paused (after "Disconnect everyone").
    Paused {
        /// When sharing resumes.
        retry_after_ms: u64,
    },
}

/// The sharing decision at `ClientInit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Let the viewer in, disconnecting these first.
    Admit {
        /// Viewers to disconnect.
        disconnect: Vec<ConnectionId>,
    },
    /// Refuse: another viewer is connected and exclusive access is not
    /// allowed.
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
    key: IpAddr,
}

/// Access decisions shared by all viewer connections of one agent.
#[derive(Debug, Clone, Default)]
pub struct AccessControl {
    policy: AccessPolicy,
    failures: HashMap<IpAddr, Failures>,
    recent_failures: VecDeque<u64>,
    global_locked_until_ms: u64,
    paused_until_ms: u64,
    connections: HashMap<ConnectionId, Connection>,
    next_id: u64,
}

/// The key lockout and caps are counted under: the address itself for IPv4
/// (including IPv4-mapped IPv6), the /64 for IPv6, since one host can use a
/// whole /64.
#[must_use]
pub fn address_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                IpAddr::V4(v4)
            } else {
                let [a, b, c, d, ..] = v6.segments();
                IpAddr::V6(Ipv6Addr::new(a, b, c, d, 0, 0, 0, 0))
            }
        }
    }
}

impl AccessControl {
    /// Creates access control with `policy`.
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

    /// Connections currently open.
    #[must_use]
    pub fn connections(&self) -> usize {
        self.connections.len()
    }

    /// A viewer at `ip` opened a connection.
    ///
    /// # Errors
    /// [`Refusal`] if the agent is paused, the address or the whole agent is
    /// locked out, or a connection cap is reached.
    pub fn connect(&mut self, ip: IpAddr, now_ms: u64) -> Result<ConnectionId, Refusal> {
        let key = address_key(ip);
        if self.paused_until_ms > now_ms {
            return Err(Refusal::Paused {
                retry_after_ms: self.paused_until_ms - now_ms,
            });
        }
        if let Some(retry_after_ms) = self.lockout_remaining(key, now_ms) {
            return Err(Refusal::LockedOut { retry_after_ms });
        }
        if self.connections.len() >= self.policy.max_connections {
            return Err(Refusal::TooManyConnections);
        }
        let pending = self.connections.values().filter(|c| !c.admitted);
        let (total, from_key) =
            pending.fold((0, 0), |(t, k), c| (t + 1, k + usize::from(c.key == key)));
        if total >= self.policy.max_pending {
            return Err(Refusal::TooManyConnections);
        }
        if from_key >= self.policy.max_pending_per_address {
            return Err(Refusal::TooManyPending);
        }
        self.next_id += 1;
        let id = ConnectionId(self.next_id);
        self.connections.insert(
            id,
            Connection {
                admitted: false,
                key,
            },
        );
        Ok(id)
    }

    /// How long `ip` (or the whole agent) is still locked out, if it is.
    #[must_use]
    pub fn lockout_remaining(&self, ip: IpAddr, now_ms: u64) -> Option<u64> {
        if self.global_locked_until_ms > now_ms {
            return Some(self.global_locked_until_ms - now_ms);
        }
        let f = self.failures.get(&address_key(ip))?;
        (f.locked_until_ms > now_ms).then(|| f.locked_until_ms - now_ms)
    }

    /// A login from `ip` failed, or its access request was denied or timed out.
    pub fn auth_failed(&mut self, ip: IpAddr, now_ms: u64) {
        let key = address_key(ip);
        self.forget_stale(now_ms);
        self.count_globally(now_ms);
        if !self.failures.contains_key(&key)
            && self.failures.len() >= self.policy.max_tracked_addresses
            && let Some(oldest) = self
                .failures
                .iter()
                .min_by_key(|(_, f)| f.last_ms)
                .map(|(&ip, _)| ip)
        {
            self.failures.remove(&oldest);
        }
        let entry = self.failures.entry(key).or_insert(Failures {
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
            tracing::warn!(address = %key, failures = entry.count, lockout_ms = lockout, "address locked out after failed logins");
        }
    }

    fn count_globally(&mut self, now_ms: u64) {
        let window = self.policy.global_window_ms;
        while self
            .recent_failures
            .front()
            .is_some_and(|&t| now_ms.saturating_sub(t) >= window)
        {
            self.recent_failures.pop_front();
        }
        self.recent_failures.push_back(now_ms);
        if self.recent_failures.len() >= self.policy.global_failure_budget {
            self.global_locked_until_ms = now_ms.saturating_add(self.policy.global_lockout_ms);
            self.recent_failures.clear();
            tracing::warn!(
                lockout_ms = self.policy.global_lockout_ms,
                "too many failed logins overall; pausing logins"
            );
        }
    }

    /// A viewer from `ip` was fully let in (owner, or approved). Only then
    /// are its address's failures forgotten, so a prompt-gated login can't
    /// reset the counter for guessing someone else's password.
    pub fn auth_succeeded(&mut self, ip: IpAddr) {
        self.failures.remove(&address_key(ip));
    }

    /// Decides whether connection `id` may proceed at `ClientInit`.
    /// Call [`Self::started`] once it is actually let in.
    pub fn admit(&mut self, id: ConnectionId, shared: bool) -> Admission {
        let others: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter(|&(&other, c)| other != id && c.admitted)
            .map(|(&other, _)| other)
            .collect();
        let exclusive = !shared && self.policy.share != SharePolicy::AlwaysShare;
        match (exclusive, others.is_empty(), self.policy.share) {
            (false, _, _) | (true, true, _) => Admission::Admit {
                disconnect: Vec::new(),
            },
            (true, false, SharePolicy::DisconnectOthers) => Admission::Admit { disconnect: others },
            (true, false, _) => Admission::Refuse,
        }
    }

    /// Connection `id` is now a viewing session (`ServerInit` sent).
    pub fn started(&mut self, id: ConnectionId) {
        if let Some(c) = self.connections.get_mut(&id) {
            c.admitted = true;
        }
    }

    /// Refuses every new connection until `until_ms` (after "Disconnect everyone").
    pub fn pause_until(&mut self, until_ms: u64) {
        self.paused_until_ms = self.paused_until_ms.max(until_ms);
    }

    /// Connection `id` closed.
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
        // Per-address behaviour only; the global budget has its own test.
        let mut a = AccessControl::new(AccessPolicy {
            global_failure_budget: usize::MAX,
            ..AccessPolicy::DEFAULT
        });
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
            let id = a.connect(IP, now + want).unwrap();
            a.disconnected(id);
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
        a.connect(OTHER, 0).unwrap();
        assert_eq!(a.connect(OTHER, 0), Err(Refusal::TooManyConnections));
        a.disconnected(first);
        assert!(a.connect(IP, 0).is_ok());
    }

    #[test]
    fn idle_strangers_cannot_take_every_slot() {
        let mut a = AccessControl::new(AccessPolicy::DEFAULT);
        // One address may hold only 2 unfinished connections.
        a.connect(IP, 0).unwrap();
        a.connect(IP, 0).unwrap();
        assert_eq!(a.connect(IP, 0), Err(Refusal::TooManyPending));
        // Many addresses together may hold at most 8: slots stay free for viewers.
        let mut pending = 2;
        for i in 10..30u8 {
            if a.connect(IpAddr::V4(Ipv4Addr::new(10, 0, 0, i)), 0).is_ok() {
                pending += 1;
            }
        }
        assert_eq!(pending, 8);
        assert_eq!(a.connections(), 8);
    }

    #[test]
    fn ipv6_addresses_in_one_64_share_a_lockout() {
        let mut a = AccessControl::new(AccessPolicy::DEFAULT);
        let one: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let other_in_same_64: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        for _ in 0..4 {
            a.auth_failed(one, 0);
        }
        assert!(a.lockout_remaining(other_in_same_64, 1).is_some());
        assert_eq!(address_key("::ffff:192.0.2.1".parse().unwrap()), IP);
    }

    #[test]
    fn rotating_addresses_hit_the_global_budget() {
        let mut a = AccessControl::new(AccessPolicy::DEFAULT);
        for i in 0..30u8 {
            a.auth_failed(IpAddr::V4(Ipv4Addr::new(10, 1, 0, i)), 1_000);
        }
        let fresh = IpAddr::V4(Ipv4Addr::new(10, 9, 9, 9));
        assert!(matches!(
            a.connect(fresh, 1_001),
            Err(Refusal::LockedOut { .. })
        ));
        assert!(a.connect(fresh, 61_001).is_ok());
    }

    #[test]
    fn pause_refuses_everyone_until_it_ends() {
        let mut a = AccessControl::new(AccessPolicy::DEFAULT);
        a.pause_until(120_000);
        assert_eq!(
            a.connect(IP, 0),
            Err(Refusal::Paused {
                retry_after_ms: 120_000
            })
        );
        assert!(a.connect(IP, 120_000).is_ok());
    }

    #[test]
    fn exclusive_requests_follow_the_share_policy() {
        let mut a = AccessControl::new(AccessPolicy::DEFAULT);
        let first = a.connect(IP, 0).unwrap();
        assert_eq!(
            a.admit(first, false),
            Admission::Admit { disconnect: vec![] }
        );
        a.started(first);
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
        kick.started(first);
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
        share.started(first);
        let second = share.connect(OTHER, 0).unwrap();
        assert_eq!(
            share.admit(second, false),
            Admission::Admit { disconnect: vec![] }
        );
    }
}
