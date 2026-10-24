//! Smithay protocol handlers: surfaces, shells, the seat, the clipboard,
//! outputs and the X11 window manager.

use std::os::fd::OwnedFd;

use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::desktop::{PopupKind, Window, find_popup_root_surface, get_popup_toplevel_coords};
use smithay::input::pointer::{CursorImageStatus, Focus};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::{
    wl_buffer, wl_output, wl_seat, wl_surface::WlSurface,
};
use smithay::utils::{IsAlive, Logical, Rectangle, Serial};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, get_parent, is_sync_subsurface,
};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
    clear_data_device_selection, current_data_device_selection_userdata,
    request_data_device_client_selection, set_data_device_focus, set_data_device_selection,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWaylandClientData, XwmHandler};
use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell, delegate_xwayland_shell,
};
use tracing::debug;

use crate::clipboard;
use crate::focus::KeyboardFocus;
use crate::grab::MoveGrab;
use crate::state::{ClientState, ClipboardOwner, State};

// --- Surfaces -------------------------------------------------------------

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // Our clients carry `ClientState`; Smithay inserts Xwayland with its own data.
        if let Some(state) = client.get_data::<XWaylandClientData>() {
            return &state.compositor_state;
        }
        match client.get_data::<ClientState>() {
            Some(state) => &state.compositor_state,
            None => unreachable_client_state(),
        }
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(&root) {
                window.on_commit();
                if let Some(toplevel) = window.toplevel()
                    && !toplevel.is_initial_configure_sent()
                {
                    toplevel.send_configure();
                }
                let drawn =
                    with_renderer_surface_state(&root, |s| s.buffer().is_some()).unwrap_or(false);
                if drawn
                    && let Some(index) = self.focus_when_drawn.iter().position(|w| *w == window)
                {
                    self.focus_when_drawn.swap_remove(index);
                    self.focus(Some(&window));
                }
            }
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
        {
            let _ = popup.send_configure();
        }
        if matches!(&self.cursor, CursorImageStatus::Surface(cursor) if cursor == surface) {
            self.cursor_dirty = true;
        }
        self.needs_render = true;
    }
}

/// Every client is ours or Xwayland; a static fallback keeps this
/// panic-free if that ever changes.
fn unreachable_client_state() -> &'static CompositorClientState {
    static FALLBACK: std::sync::OnceLock<CompositorClientState> = std::sync::OnceLock::new();
    FALLBACK.get_or_init(CompositorClientState::default)
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_compositor!(State);
delegate_shm!(State);

// --- Wayland windows (xdg-shell) -----------------------------------------

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        let at = self.next_window_location();
        self.space.map_element(window.clone(), at, false);
        // Focus once it has drawn something: some apps crash if the keyboard
        // arrives before their first configure.
        self.focus_when_drawn.push(window);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.space.unmap_elem(&window);
        }
        self.focus_when_drawn.retain(IsAlive::alive);
        self.focus_topmost();
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::<State>::from_resource(&seat) else {
            return;
        };
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.start_move(&seat, &window, serial);
        }
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
        // Interactive resizing isn't supported; windows keep the size they chose.
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.fill_screen(&surface, xdg_toplevel::State::Maximized);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        Self::unfill_screen(&surface, xdg_toplevel::State::Maximized);
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<wl_output::WlOutput>,
    ) {
        self.fill_screen(&surface, xdg_toplevel::State::Fullscreen);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        Self::unfill_screen(&surface, xdg_toplevel::State::Fullscreen);
    }
}

delegate_xdg_shell!(State);

impl State {
    /// Gives the keyboard to the topmost real window that is still alive
    /// (not a menu or tooltip), or to nobody.
    fn focus_topmost(&mut self) {
        let next = self
            .space
            .elements()
            .rev()
            .find(|w| {
                w.alive()
                    && !w
                        .x11_surface()
                        .is_some_and(X11Surface::is_override_redirect)
            })
            .cloned();
        self.focus(next.as_ref());
    }

    /// The monitor a window is on (or the first one), as a rectangle.
    fn screen_of(&self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let outputs = self.space.outputs_for_element(window);
        let output = outputs.first().or_else(|| self.space.outputs().next())?;
        self.space.output_geometry(output)
    }

    /// Maximise or full-screen: the window covers its monitor.
    fn fill_screen(&mut self, surface: &ToplevelSurface, how: xdg_toplevel::State) {
        if let Some(window) = self.window_for_surface(surface.wl_surface())
            && let Some(screen) = self.screen_of(&window)
        {
            surface.with_pending_state(|state| {
                state.states.set(how);
                state.size = Some(screen.size);
            });
            self.move_window(&window, screen.loc);
        }
        // The protocol always wants a configure in reply.
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn unfill_screen(surface: &ToplevelSurface, how: xdg_toplevel::State) {
        surface.with_pending_state(|state| {
            state.states.unset(how);
            state.size = None;
        });
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn start_move(&mut self, seat: &Seat<State>, window: &Window, serial: Serial) {
        let Some(pointer) = seat.get_pointer() else {
            return;
        };
        if !pointer.has_grab(serial) {
            return;
        }
        let Some(start_data) = pointer.grab_start_data() else {
            return;
        };
        let Some(initial_location) = self.space.element_location(window) else {
            return;
        };
        let grab = MoveGrab {
            start_data,
            window: window.clone(),
            initial_location,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// Keeps popups (menus) on the desktop.
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self.window_for_surface(&root) else {
            return;
        };
        let Some(window_geometry) = self.space.element_geometry(&window) else {
            return;
        };
        let outputs = self.space.outputs_for_element(&window);
        let Some(output) = outputs.first().or_else(|| self.space.outputs().next()) else {
            return;
        };
        let Some(mut target) = self.space.output_geometry(output) else {
            return;
        };
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geometry.loc;
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

// --- Seat -------------------------------------------------------------------

impl SeatHandler for State {
    type KeyboardFocus = KeyboardFocus;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor = image;
        self.cursor_dirty = true;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KeyboardFocus>) {
        let client = focused
            .and_then(WaylandFocus::wl_surface)
            .and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, seat, client);
    }
}

delegate_seat!(State);

// --- Clipboard ------------------------------------------------------------

impl SelectionHandler for State {
    type SelectionUserData = ClipboardOwner;

    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        seat: Seat<Self>,
    ) {
        if ty != SelectionTarget::Clipboard {
            return;
        }
        let types = source.as_ref().map(SelectionSource::mime_types);
        if let Some(xwm) = self.xwm.as_mut()
            && let Err(error) = xwm.new_selection(ty, types.clone())
        {
            debug!(%error, "could not pass the clipboard to X11 apps");
        }
        // A Wayland app copied something: send the text to viewers.
        if let Some(mime) = types.as_deref().and_then(clipboard::text_type)
            && let Ok(fd) = clipboard::reader_for_viewer(self.events.clone())
        {
            let _ = request_data_device_client_selection(&seat, mime, fd);
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        owner: &ClipboardOwner,
    ) {
        match owner {
            ClipboardOwner::Viewer(text) => clipboard::write_text(text.clone(), fd),
            ClipboardOwner::Xwayland => {
                if let Some(xwm) = self.xwm.as_mut() {
                    let _ = xwm.send_selection(ty, mime_type, fd, self.loop_handle.clone());
                }
            }
        }
    }
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

delegate_data_device!(State);

// --- Outputs ----------------------------------------------------------------

impl OutputHandler for State {}
delegate_output!(State);

// --- X11 windows (Xwayland) -------------------------------------------------

impl XWaylandShellHandler for State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

delegate_xwayland_shell!(State);

impl State {
    fn fill_screen_x11(&mut self, surface: &X11Surface) {
        if let Some(window) = self.x11_window(surface)
            && let Some(screen) = self.screen_of(&window)
        {
            let _ = surface.configure(Some(screen));
            self.space.map_element(window, screen.loc, true);
            self.needs_render = true;
        }
    }

    fn x11_window(&self, surface: &X11Surface) -> Option<Window> {
        self.space
            .elements()
            .find(|w| w.x11_surface() == Some(surface))
            .cloned()
    }
}

impl XwmHandler for State {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        // Smithay only calls this while the WM it started is running.
        self.xwm
            .as_mut()
            .unwrap_or_else(|| unreachable!("X11 event without a window manager"))
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if surface.set_mapped(true).is_err() {
            return;
        }
        let window = Window::new_x11_window(surface.clone());
        let at = self.next_window_location();
        self.space.map_element(window.clone(), at, false);
        let mut geometry = surface.geometry();
        geometry.loc = at;
        let _ = surface.configure(Some(geometry));
        self.focus(Some(&window));
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        // Menus and tooltips place themselves.
        let at = surface.geometry().loc;
        self.space
            .map_element(Window::new_x11_window(surface), at, true);
        self.needs_render = true;
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(window) = self.x11_window(&surface) {
            self.space.unmap_elem(&window);
        }
        let had_focus = self
            .seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .is_some_and(|f| matches!(f, KeyboardFocus::X11(w) if w == surface));
        if !surface.is_override_redirect() {
            let _ = surface.set_mapped(false);
        }
        if had_focus {
            self.focus_topmost();
        }
        self.needs_render = true;
    }

    fn destroyed_window(&mut self, _xwm: XwmId, _surface: X11Surface) {}

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // Windows may pick their size; the compositor picks their place.
        let mut geometry = surface.geometry();
        if let Some(w) = w.and_then(|w| i32::try_from(w).ok()) {
            geometry.size.w = w;
        }
        if let Some(h) = h.and_then(|h| i32::try_from(h).ok()) {
            geometry.size.h = h;
        }
        let _ = surface.configure(geometry);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        if let Some(window) = self.x11_window(&surface) {
            self.space.map_element(window, geometry.loc, false);
            self.needs_render = true;
        }
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _surface: X11Surface,
        _button: u32,
        _edges: ResizeEdge,
    ) {
    }

    fn maximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let _ = surface.set_maximized(true);
        self.fill_screen_x11(&surface);
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let _ = surface.set_fullscreen(true);
        self.fill_screen_x11(&surface);
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let _ = surface.set_maximized(false);
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        let _ = surface.set_fullscreen(false);
    }

    fn move_request(&mut self, _xwm: XwmId, surface: X11Surface, _button: u32) {
        let Some(window) = self.x11_window(&surface) else {
            return;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let Some(start_data) = pointer.grab_start_data() else {
            return;
        };
        let Some(initial_location) = self.space.element_location(&window) else {
            return;
        };
        let grab = MoveGrab {
            start_data,
            window,
            initial_location,
        };
        pointer.set_grab(
            self,
            grab,
            smithay::utils::SERIAL_COUNTER.next_serial(),
            Focus::Clear,
        );
    }

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        // Only the focused X11 app may read the clipboard.
        self.seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .is_some_and(|focus| matches!(focus, KeyboardFocus::X11(w) if w.xwm_id() == Some(xwm)))
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        if selection != SelectionTarget::Clipboard {
            return;
        }
        let viewer_text = match current_data_device_selection_userdata(&self.seat).as_deref() {
            Some(ClipboardOwner::Viewer(text)) => Some(text.clone()),
            _ => None,
        };
        match viewer_text {
            Some(text) => clipboard::write_text(text, fd),
            None => {
                let _ = request_data_device_client_selection(&self.seat, mime_type, fd);
            }
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        if selection != SelectionTarget::Clipboard {
            return;
        }
        set_data_device_selection(
            &self.display_handle,
            &self.seat,
            mime_types.clone(),
            ClipboardOwner::Xwayland,
        );
        // An X11 app copied something: send the text to viewers.
        if let Some(mime) = clipboard::text_type(&mime_types)
            && let Ok(fd) = clipboard::reader_for_viewer(self.events.clone())
            && let Some(xwm) = self.xwm.as_mut()
        {
            let _ = xwm.send_selection(selection, mime, fd, self.loop_handle.clone());
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        if selection == SelectionTarget::Clipboard
            && matches!(
                current_data_device_selection_userdata(&self.seat).as_deref(),
                Some(ClipboardOwner::Xwayland)
            )
        {
            clear_data_device_selection(&self.display_handle, &self.seat);
        }
    }
}
