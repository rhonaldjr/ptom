//! The xdg-desktop-portal side: one RemoteDesktop session with a ScreenCast source, the input
//! it injects, and the task that owns the session for as long as the capture runs. Desktops
//! whose portal can't inject input (no RemoteDesktop) get a ScreenCast-only session: the
//! screen is shared view-only and input is dropped.

use std::os::fd::OwnedFd;
use std::sync::Arc;

use ashpd::desktop::remote_desktop::{
    Axis, DeviceType, KeyState, NotifyKeyboardKeycodeOptions, NotifyKeyboardKeysymOptions,
    NotifyPointerAxisDiscreteOptions, NotifyPointerButtonOptions,
    NotifyPointerMotionAbsoluteOptions, RemoteDesktop, SelectDevicesOptions, StartOptions,
};
use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, ResponseError, Session};
use ashpd::enumflags2::BitFlags;
use futures_util::StreamExt;
use tokio::sync::mpsc;

use super::FrameSize;
use super::stream::PipeWire;
use crate::pointer::{self, PointerAction, ScrollAxis};
use crate::{CaptureError, CaptureEvent, InputEvent, Options};

/// An open, started portal session.
pub(super) struct Portal {
    remote: RemoteDesktop,
    session: Session<RemoteDesktop>,
}

/// The portal session behind a capture.
pub(super) enum Sharing {
    /// Screen and input (RemoteDesktop with a ScreenCast source).
    Full(Portal),
    /// Screen only: this desktop's portal can't inject input.
    ScreenOnly(Session<Screencast>),
}

impl Sharing {
    /// Ends the session (failure only logged: it dies with our D-Bus connection anyway).
    pub async fn close(&self) {
        let result = match self {
            Self::Full(portal) => portal.session.close().await,
            Self::ScreenOnly(session) => session.close().await,
        };
        if let Err(error) = result {
            tracing::debug!("closing the portal session failed: {error}");
        }
    }

    /// Resolves when the desktop closes the session (e.g. the person stops sharing).
    async fn closed(&self) {
        match self {
            Self::Full(portal) => wait_closed(&portal.session).await,
            Self::ScreenOnly(session) => wait_closed(session).await,
        }
    }
}

async fn wait_closed<T: ashpd::desktop::SessionPortal>(session: &Session<T>) {
    match session.receive_closed().await {
        Ok(signals) => {
            let mut signals = std::pin::pin!(signals);
            signals.next().await;
        }
        Err(error) => {
            tracing::debug!("cannot watch the portal session: {error}");
            std::future::pending::<()>().await;
        }
    }
}

/// What [`open`] hands back.
pub(super) struct Opened {
    pub portal: Sharing,
    pub pipewire_fd: OwnedFd,
    pub node_id: u32,
    /// The stream's size in the portal's logical coordinates, if it said.
    pub logical_size: Option<(u32, u32)>,
    pub restore_token: Option<String>,
}

/// The running desktop's name for error messages.
fn desktop_name() -> String {
    std::env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn missing(portal: &'static str) -> CaptureError {
    CaptureError::PortalMissing {
        portal,
        desktop: desktop_name(),
    }
}

fn portal_error(error: ashpd::Error) -> CaptureError {
    match error {
        ashpd::Error::Response(ResponseError::Cancelled) => CaptureError::Declined,
        ashpd::Error::PortalNotFound(name) if name.as_str().ends_with("RemoteDesktop") => {
            missing("RemoteDesktop")
        }
        ashpd::Error::PortalNotFound(_) => missing("ScreenCast"),
        other => CaptureError::Portal(other.to_string()),
    }
}

/// Opens the portals and starts the session: screen and input when the desktop allows it,
/// otherwise the screen alone (view-only, with a warning).
pub(super) async fn open(options: &Options) -> Result<Opened, CaptureError> {
    match open_full(options).await {
        Err(
            CaptureError::PortalMissing {
                portal: "RemoteDesktop",
                desktop,
            }
            | CaptureError::DevicesUnavailable { desktop },
        ) => {
            tracing::warn!(
                "{desktop}'s portal can't let apps control the keyboard and mouse; \
                 sharing the screen view-only"
            );
            open_screen_only(options).await
        }
        other => other,
    }
}

/// A RemoteDesktop session: keyboard, pointer and one monitor.
async fn open_full(options: &Options) -> Result<Opened, CaptureError> {
    let remote = RemoteDesktop::new()
        .await
        .map_err(|_| missing("RemoteDesktop"))?;
    // A portal frontend without a RemoteDesktop backend fails here rather than in `new`.
    let devices = remote
        .available_device_types()
        .await
        .map_err(|_| missing("RemoteDesktop"))?;
    if !devices.contains(DeviceType::Keyboard | DeviceType::Pointer) {
        return Err(CaptureError::DevicesUnavailable {
            desktop: desktop_name(),
        });
    }
    let (screencast, cursor_mode) = screencast().await?;
    let session = remote
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(portal_error)?;
    let portal = Portal { remote, session };
    match portal.negotiate(&screencast, options, cursor_mode).await {
        Ok((negotiated, pipewire_fd)) => Ok(Opened {
            portal: Sharing::Full(portal),
            pipewire_fd,
            node_id: negotiated.node_id,
            logical_size: negotiated.logical_size,
            restore_token: negotiated.restore_token,
        }),
        Err(error) => {
            Sharing::Full(portal).close().await;
            Err(error)
        }
    }
}

/// The ScreenCast portal and the best cursor mode it offers.
async fn screencast() -> Result<(Screencast, CursorMode), CaptureError> {
    let screencast = Screencast::new().await.map_err(|_| missing("ScreenCast"))?;
    let cursor_modes = screencast
        .available_cursor_modes()
        .await
        .map_err(|_| missing("ScreenCast"))?;
    let cursor_mode = if cursor_modes.contains(CursorMode::Metadata) {
        CursorMode::Metadata
    } else {
        CursorMode::Embedded
    };
    Ok((screencast, cursor_mode))
}

/// A ScreenCast-only session: one monitor, no input.
async fn open_screen_only(options: &Options) -> Result<Opened, CaptureError> {
    let (screencast, cursor_mode) = screencast().await?;
    let session = screencast
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(portal_error)?;
    let persist = if options.persist {
        PersistMode::ExplicitlyRevoked
    } else {
        PersistMode::DoNot
    };
    let started = async {
        screencast
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_sources(BitFlags::from(SourceType::Monitor))
                    .set_multiple(false)
                    .set_cursor_mode(cursor_mode)
                    .set_persist_mode(persist)
                    .set_restore_token(options.restore_token.as_deref()),
            )
            .await
            .and_then(|request| request.response())
            .map_err(portal_error)?;
        let selected = screencast
            .start(&session, None, StartCastOptions::default())
            .await
            .and_then(|request| request.response())
            .map_err(portal_error)?;
        let stream = selected.streams().first().ok_or(CaptureError::NoStream)?;
        let logical_size = stream.size().and_then(|(width, height)| {
            Some((u32::try_from(width).ok()?, u32::try_from(height).ok()?))
        });
        let negotiated = Negotiated {
            node_id: stream.pipe_wire_node_id(),
            logical_size: logical_size.filter(|&(w, h)| w > 0 && h > 0),
            restore_token: selected.restore_token().map(str::to_owned),
        };
        let fd = screencast
            .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
            .await
            .map_err(portal_error)?;
        Ok::<_, CaptureError>((negotiated, fd))
    }
    .await;
    match started {
        Ok((negotiated, pipewire_fd)) => Ok(Opened {
            portal: Sharing::ScreenOnly(session),
            pipewire_fd,
            node_id: negotiated.node_id,
            logical_size: negotiated.logical_size,
            restore_token: negotiated.restore_token,
        }),
        Err(error) => {
            Sharing::ScreenOnly(session).close().await;
            Err(error)
        }
    }
}

struct Negotiated {
    node_id: u32,
    logical_size: Option<(u32, u32)>,
    restore_token: Option<String>,
}

impl Portal {
    async fn negotiate(
        &self,
        screencast: &Screencast,
        options: &Options,
        cursor_mode: CursorMode,
    ) -> Result<(Negotiated, OwnedFd), CaptureError> {
        let persist = if options.persist {
            PersistMode::ExplicitlyRevoked
        } else {
            PersistMode::DoNot
        };
        // Persistence for a remote desktop session is requested here, not in SelectSources.
        self.remote
            .select_devices(
                &self.session,
                SelectDevicesOptions::default()
                    .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                    .set_persist_mode(persist)
                    .set_restore_token(options.restore_token.as_deref()),
            )
            .await
            .and_then(|request| request.response())
            .map_err(portal_error)?;
        screencast
            .select_sources(
                &self.session,
                SelectSourcesOptions::default()
                    .set_sources(BitFlags::from(SourceType::Monitor))
                    .set_multiple(false)
                    .set_cursor_mode(cursor_mode),
            )
            .await
            .and_then(|request| request.response())
            .map_err(portal_error)?;
        let selected = self
            .remote
            .start(&self.session, None, StartOptions::default())
            .await
            .and_then(|request| request.response())
            .map_err(portal_error)?;
        if !selected
            .devices()
            .contains(DeviceType::Keyboard | DeviceType::Pointer)
        {
            tracing::warn!("the portal granted screen sharing without full keyboard and pointer");
        }
        let stream = selected.streams().first().ok_or(CaptureError::NoStream)?;
        let logical_size = stream.size().and_then(|(width, height)| {
            Some((u32::try_from(width).ok()?, u32::try_from(height).ok()?))
        });
        let negotiated = Negotiated {
            node_id: stream.pipe_wire_node_id(),
            logical_size: logical_size.filter(|&(w, h)| w > 0 && h > 0),
            restore_token: selected.restore_token().map(str::to_owned),
        };
        let fd = screencast
            .open_pipe_wire_remote(&self.session, OpenPipeWireRemoteOptions::default())
            .await
            .map_err(portal_error)?;
        Ok((negotiated, fd))
    }
}

/// Turns [`InputEvent`]s into portal calls, remembering the button mask and position.
struct Injector {
    node_id: u32,
    logical_size: Option<(u32, u32)>,
    frame_size: Arc<FrameSize>,
    buttons: u8,
    position: Option<(u16, u16)>,
    /// Keys pressed and not released yet, so they can be let go when a
    /// viewer disappears (else they stay held on the user's own desktop).
    held_keysyms: std::collections::HashSet<i32>,
    held_codes: std::collections::HashSet<u16>,
}

impl Injector {
    async fn inject(&mut self, portal: &Portal, event: InputEvent) -> Result<(), ashpd::Error> {
        match event {
            InputEvent::Key { down, keysym } => {
                let Ok(keysym) = i32::try_from(keysym) else {
                    return Ok(());
                };
                if down {
                    self.held_keysyms.insert(keysym);
                } else {
                    self.held_keysyms.remove(&keysym);
                }
                let state = if down {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                };
                portal
                    .remote
                    .notify_keyboard_keysym(
                        &portal.session,
                        keysym,
                        state,
                        NotifyKeyboardKeysymOptions::default(),
                    )
                    .await
            }
            InputEvent::KeyCode { down, evdev } => {
                if down {
                    self.held_codes.insert(evdev);
                } else {
                    self.held_codes.remove(&evdev);
                }
                let state = if down {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                };
                portal
                    .remote
                    .notify_keyboard_keycode(
                        &portal.session,
                        i32::from(evdev),
                        state,
                        NotifyKeyboardKeycodeOptions::default(),
                    )
                    .await
            }
            InputEvent::ReleaseAll => {
                for keysym in std::mem::take(&mut self.held_keysyms) {
                    portal
                        .remote
                        .notify_keyboard_keysym(
                            &portal.session,
                            keysym,
                            KeyState::Released,
                            NotifyKeyboardKeysymOptions::default(),
                        )
                        .await?;
                }
                for evdev in std::mem::take(&mut self.held_codes) {
                    portal
                        .remote
                        .notify_keyboard_keycode(
                            &portal.session,
                            i32::from(evdev),
                            KeyState::Released,
                            NotifyKeyboardKeycodeOptions::default(),
                        )
                        .await?;
                }
                let previous = std::mem::take(&mut self.buttons);
                for action in pointer::actions(previous, 0) {
                    Self::act(portal, action).await?;
                }
                Ok(())
            }
            InputEvent::Pointer { buttons, x, y } => {
                if self.position != Some((x, y)) {
                    self.position = Some((x, y));
                    self.move_to(portal, x, y).await?;
                }
                let previous = std::mem::replace(&mut self.buttons, buttons);
                for action in pointer::actions(previous, buttons) {
                    Self::act(portal, action).await?;
                }
                Ok(())
            }
        }
    }

    async fn move_to(&self, portal: &Portal, x: u16, y: u16) -> Result<(), ashpd::Error> {
        let (frame_w, frame_h) = self.frame_size.get();
        let (to_w, to_h) = self.logical_size.unwrap_or((frame_w, frame_h));
        portal
            .remote
            .notify_pointer_motion_absolute(
                &portal.session,
                self.node_id,
                pointer::scale(x, frame_w, to_w),
                pointer::scale(y, frame_h, to_h),
                NotifyPointerMotionAbsoluteOptions::default(),
            )
            .await
    }

    async fn act(portal: &Portal, action: PointerAction) -> Result<(), ashpd::Error> {
        match action {
            PointerAction::Button { code, pressed } => {
                let state = if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                };
                portal
                    .remote
                    .notify_pointer_button(
                        &portal.session,
                        code,
                        state,
                        NotifyPointerButtonOptions::default(),
                    )
                    .await
            }
            PointerAction::Scroll { axis, steps } => {
                let axis = match axis {
                    ScrollAxis::Vertical => Axis::Vertical,
                    ScrollAxis::Horizontal => Axis::Horizontal,
                };
                portal
                    .remote
                    .notify_pointer_axis_discrete(
                        &portal.session,
                        axis,
                        steps,
                        NotifyPointerAxisDiscreteOptions::default(),
                    )
                    .await
            }
        }
    }
}

/// Why the supervisor stopped.
enum Stop {
    /// The caller dropped the event receiver.
    ReceiverGone,
    /// The PipeWire thread ended on its own, with this reason.
    PipeWireEnded(Option<String>),
    /// The desktop closed the portal session (e.g. the person stopped sharing).
    PortalClosed,
}

/// Everything the supervisor owns.
pub(super) struct Supervisor {
    pub opened_portal: Sharing,
    pub node_id: u32,
    pub logical_size: Option<(u32, u32)>,
    pub frame_size: Arc<FrameSize>,
    pub input: mpsc::Receiver<InputEvent>,
    pub events: mpsc::Sender<CaptureEvent>,
    pub pipewire: PipeWire,
}

impl Supervisor {
    /// Injects input until the capture ends, then tears everything down. It is the only sender
    /// of [`CaptureEvent::Closed`], so the caller sees it exactly once.
    pub async fn run(self) {
        let Self {
            opened_portal: portal,
            node_id,
            logical_size,
            frame_size,
            mut input,
            events,
            pipewire,
        } = self;
        let PipeWire { quit, mut done } = pipewire;
        let mut injector = Injector {
            node_id,
            logical_size,
            frame_size,
            buttons: 0,
            position: None,
            held_keysyms: std::collections::HashSet::new(),
            held_codes: std::collections::HashSet::new(),
        };
        let session_closed = portal.closed();
        let mut session_closed = std::pin::pin!(session_closed);
        let mut input_open = true;

        let stop = loop {
            tokio::select! {
                event = input.recv(), if input_open => match event {
                    Some(event) => {
                        // Screen-only sessions can't inject: input is dropped.
                        if let Sharing::Full(full) = &portal
                            && let Err(error) = injector.inject(full, event).await
                        {
                            // The error text never includes the event (it may be a keystroke).
                            tracing::debug!("the portal rejected an input event: {error}");
                        }
                    }
                    None => input_open = false,
                },
                () = events.closed() => break Stop::ReceiverGone,
                reason = &mut done => break Stop::PipeWireEnded(reason.ok().flatten()),
                () = &mut session_closed => break Stop::PortalClosed,
            }
        };

        let reason = match stop {
            Stop::PipeWireEnded(reason) => {
                Some(reason.unwrap_or_else(|| "the screen stream ended".to_owned()))
            }
            Stop::ReceiverGone | Stop::PortalClosed => {
                let _ = quit.send(());
                // Wait for the thread so no frame can follow the Closed event.
                let _ = done.await;
                matches!(stop, Stop::PortalClosed)
                    .then(|| "screen sharing was stopped at the remote desktop".to_owned())
            }
        };
        portal.close().await;
        if let Some(reason) = reason {
            tracing::info!("Wayland capture closed: {reason}");
            let _ = events.send(CaptureEvent::Closed(reason)).await;
        }
    }
}
