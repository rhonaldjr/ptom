//! Keyboard focus: a Wayland surface, or an X11 window (which also needs X
//! input focus, handled by Smithay's `X11Surface`).

use std::borrow::Cow;

use smithay::backend::input::KeyState;
use smithay::input::Seat;
use smithay::input::keyboard::{KeyboardTarget, KeysymHandle, ModifiersState};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Serial};
use smithay::wayland::seat::WaylandFocus;
use smithay::xwayland::X11Surface;

use crate::state::State;

/// What has the keyboard.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum KeyboardFocus {
    Wayland(WlSurface),
    X11(X11Surface),
}

impl WaylandFocus for KeyboardFocus {
    /// The Wayland surface behind the focus (X11 windows have one once mapped).
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Wayland(surface) => Some(Cow::Borrowed(surface)),
            Self::X11(window) => window.wl_surface().map(Cow::Owned),
        }
    }
}

impl IsAlive for KeyboardFocus {
    fn alive(&self) -> bool {
        match self {
            Self::Wayland(surface) => surface.alive(),
            Self::X11(window) => window.alive(),
        }
    }
}

impl KeyboardTarget<State> for KeyboardFocus {
    fn enter(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(s) => KeyboardTarget::enter(s, seat, data, keys, serial),
            Self::X11(w) => KeyboardTarget::enter(w, seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {
        match self {
            Self::Wayland(s) => KeyboardTarget::leave(s, seat, data, serial),
            Self::X11(w) => KeyboardTarget::leave(w, seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match self {
            Self::Wayland(s) => KeyboardTarget::key(s, seat, data, key, state, serial, time),
            Self::X11(w) => KeyboardTarget::key(w, seat, data, key, state, serial, time),
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(s) => KeyboardTarget::modifiers(s, seat, data, modifiers, serial),
            Self::X11(w) => KeyboardTarget::modifiers(w, seat, data, modifiers, serial),
        }
    }
}
