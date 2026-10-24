//! Who controls a shared desktop (user decision U-9): one viewer at a time.
//!
//! The first viewer to send input gets control. Others only watch until the
//! controller leaves or stops using the mouse and keyboard for a while (a
//! quiet hand-over), or until the desktop's owner sends input: the owner can
//! always take over.

use crate::ConnectionId;

/// How long the controller must be idle before someone else can take over.
pub const DEFAULT_HANDOVER_AFTER_MS: u64 = 10_000;

/// The single-controller rule for one shared desktop.
#[derive(Debug, Clone)]
pub struct Control {
    controller: Option<Controller>,
    handover_after_ms: u64,
}

#[derive(Debug, Clone, Copy)]
struct Controller {
    id: ConnectionId,
    last_input_ms: u64,
}

/// Who is asking for control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Logged in as the desktop's owner: may always take over.
    Owner,
    /// Anyone else.
    Guest,
}

/// What [`Control::request`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The viewer already had control.
    Allowed,
    /// The viewer now has control (it was free, idle, or the owner took it).
    TookOver {
        /// Who had it before, if anyone.
        from: Option<ConnectionId>,
    },
    /// Someone else has control; drop this input.
    Refused,
}

impl Default for Control {
    fn default() -> Self {
        Self::new(DEFAULT_HANDOVER_AFTER_MS)
    }
}

impl Control {
    /// No controller yet; control passes after `handover_after_ms` of idleness.
    #[must_use]
    pub fn new(handover_after_ms: u64) -> Self {
        Self {
            controller: None,
            handover_after_ms,
        }
    }

    /// Viewer `id`, with `role`, sent input at `now_ms`.
    pub fn request(&mut self, id: ConnectionId, role: Role, now_ms: u64) -> Decision {
        let owner = role == Role::Owner;
        let decision = match self.controller {
            Some(current) if current.id == id => Decision::Allowed,
            None => Decision::TookOver { from: None },
            Some(current)
                if owner
                    || now_ms.saturating_sub(current.last_input_ms) >= self.handover_after_ms =>
            {
                Decision::TookOver {
                    from: Some(current.id),
                }
            }
            Some(_) => Decision::Refused,
        };
        if decision != Decision::Refused {
            self.controller = Some(Controller {
                id,
                last_input_ms: now_ms,
            });
        }
        decision
    }

    /// Viewer `id` left; control becomes free if it had it.
    pub fn disconnected(&mut self, id: ConnectionId) {
        if self.controller.is_some_and(|c| c.id == id) {
            self.controller = None;
        }
    }

    /// Who has control now.
    #[must_use]
    pub fn controller(&self) -> Option<ConnectionId> {
        self.controller.map(|c| c.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> ConnectionId {
        ConnectionId::from_raw(n)
    }

    #[test]
    fn the_first_viewer_to_act_controls_and_others_watch() {
        let mut control = Control::new(1000);
        assert_eq!(
            control.request(id(1), Role::Guest, 0),
            Decision::TookOver { from: None }
        );
        assert_eq!(control.request(id(2), Role::Guest, 10), Decision::Refused);
        assert_eq!(control.request(id(1), Role::Guest, 20), Decision::Allowed);
        assert_eq!(control.controller(), Some(id(1)));
    }

    #[test]
    fn control_passes_when_the_controller_is_idle_or_leaves() {
        let mut control = Control::new(1000);
        control.request(id(1), Role::Guest, 0);
        assert_eq!(control.request(id(2), Role::Guest, 999), Decision::Refused);
        assert_eq!(
            control.request(id(2), Role::Guest, 1000),
            Decision::TookOver { from: Some(id(1)) }
        );
        control.disconnected(id(2));
        assert_eq!(control.controller(), None);
        assert_eq!(
            control.request(id(3), Role::Guest, 1001),
            Decision::TookOver { from: None }
        );
    }

    #[test]
    fn the_owner_can_always_take_over() {
        let mut control = Control::new(1000);
        control.request(id(1), Role::Guest, 0);
        assert_eq!(
            control.request(id(2), Role::Owner, 1),
            Decision::TookOver { from: Some(id(1)) }
        );
        assert_eq!(control.request(id(1), Role::Guest, 2), Decision::Refused);
    }
}
