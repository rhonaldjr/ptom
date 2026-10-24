//! One open connection: its prompts, its desktop and its input (M6: each
//! lives in a tab, and can be moved to its own window).

use std::collections::HashMap;

use cosmic::app::Task;
use cosmic::iced;
use cosmic::iced::futures::SinkExt;
use cosmic::iced::keyboard::{Key, Location, key::Named, key::Physical};
use cosmic::iced::{Alignment, Length, Point, Size, mouse, window};
use cosmic::prelude::*;
use cosmic::widget::{self, image};
use farsight_client::{Fingerprint, IdentityKind, ServerAddress, SessionConfig, SessionEvent};
use farsight_net::{Command, ConnectionHandle, CursorImage, Update};
use farsight_rfb::SecurityType;
use secrecy::SecretString;

use crate::app::Message;
use crate::keymap;
use crate::store::{Remember, Store};
use crate::views::{CursorOverlay, Mapping, centred, format_fingerprint};

/// Identifies a tab for the lifetime of the app.
pub type TabId = u64;

/// Keysyms sent by the "Send Ctrl+Alt+Del" menu entry.
const CTRL_ALT_DEL: [u32; 3] = [0xffe3, 0xffe9, 0xffff];
/// Wheel distance, in pixels, that counts as one notch.
const PIXELS_PER_NOTCH: f32 = 50.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabScreen {
    Connecting,
    Credentials {
        username: bool,
    },
    Identity {
        fingerprint: Fingerprint,
        kind: IdentityKind,
        changed: bool,
    },
    InsecureWarning(SecurityType),
    Desktop,
    Failed(String),
}

/// What identifies one connection attempt (drives the subscription).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ConnectRequest {
    pub tab: TabId,
    pub host: String,
    pub port: u16,
    pub view_only: bool,
    pub shared: bool,
    pub quality: Option<u8>,
    pub compress: Option<u8>,
    pub allow_insecure: bool,
    pub pinned: Option<Fingerprint>,
}

#[derive(Debug, Clone)]
pub enum Net {
    Ready(ConnectionHandle),
    Update(Update),
    Failed(String),
}

/// Messages handled by one tab.
#[derive(Debug, Clone)]
pub enum TabMessage {
    UsernameChanged(String),
    PasswordChanged(String),
    TogglePasswordVisibility,
    ToggleRemember(bool),
    SubmitCredentials,
    AcknowledgeChange(bool),
    TrustIdentity,
    AcceptInsecure,
    Close,
    ToggleMenu,
    ToggleFullscreen,
    ToggleViewOnly,
    SendCtrlAltDel,
    MoveToWindow,
    PointerMoved { remote: (u16, u16), local: Point },
}

/// What the app must do after a tab handled something.
pub enum Outcome {
    None,
    Task(Task<Message>),
    /// The user closed the tab or the connection ended normally.
    Close,
    /// Open this tab in its own window.
    Detach,
}

#[derive(Debug, Clone)]
pub struct Connected {
    pub name: String,
    pub security: SecurityType,
    pub encrypted: bool,
}

#[allow(clippy::struct_excessive_bools)] // Independent UI toggles.
pub struct Tab {
    pub id: TabId,
    pub address: ServerAddress,
    pub screen: TabScreen,
    pub request: ConnectRequest,
    pub connected: Option<Connected>,
    pub window: Option<window::Id>,
    pub pending_resize: Option<(u16, u16)>,
    pub full_screen: bool,
    handle: Option<ConnectionHandle>,
    frame: Option<(image::Handle, u32, u32)>,
    cursor: Option<(image::Handle, CursorImage)>,
    local_pointer: Option<Point>,
    view_only: bool,
    menu_open: bool,
    buttons: u8,
    pointer: (u16, u16),
    scroll_accumulator: (f32, f32),
    pressed_keys: HashMap<Physical, u32>,
    stored_credentials: Option<Remember>,
    pending_remember: Option<Remember>,
    username_input: String,
    password_input: String,
    password_hidden: bool,
    remember: bool,
    change_acknowledged: bool,
    menu_key_f8: bool,
}

impl Tab {
    pub fn new(
        request: ConnectRequest,
        address: ServerAddress,
        stored_credentials: Option<Remember>,
        menu_key_f8: bool,
    ) -> Self {
        Self {
            id: request.tab,
            view_only: request.view_only,
            address,
            screen: TabScreen::Connecting,
            request,
            connected: None,
            window: None,
            pending_resize: None,
            full_screen: false,
            handle: None,
            frame: None,
            cursor: None,
            local_pointer: None,
            menu_open: false,
            buttons: 0,
            pointer: (0, 0),
            scroll_accumulator: (0.0, 0.0),
            pressed_keys: HashMap::new(),
            stored_credentials,
            pending_remember: None,
            username_input: String::new(),
            password_input: String::new(),
            password_hidden: true,
            remember: false,
            change_acknowledged: false,
            menu_key_f8,
        }
    }

    /// Name shown on the tab and window title.
    pub fn title(&self) -> String {
        self.connected
            .as_ref()
            .map_or_else(|| self.address.display_name(), |c| c.name.clone())
    }

    /// Whether the desktop is live and has a picture (input goes here).
    pub fn is_desktop(&self) -> bool {
        self.screen == TabScreen::Desktop
    }

    pub fn command(&self, command: Command) {
        if let Some(handle) = &self.handle {
            // A closed connection reports itself through `Update::Closed`.
            let _ = handle.send(command);
        }
    }

    pub fn update(&mut self, message: TabMessage, window: Option<window::Id>) -> Outcome {
        match message {
            TabMessage::UsernameChanged(text) => self.username_input = text,
            TabMessage::PasswordChanged(text) => self.password_input = text,
            TabMessage::TogglePasswordVisibility => self.password_hidden = !self.password_hidden,
            TabMessage::ToggleRemember(remember) => self.remember = remember,
            TabMessage::SubmitCredentials => self.submit_credentials(),
            TabMessage::AcknowledgeChange(acknowledged) => self.change_acknowledged = acknowledged,
            TabMessage::TrustIdentity => {
                self.command(Command::ConfirmIdentity);
                self.screen = TabScreen::Connecting;
            }
            TabMessage::AcceptInsecure => {
                self.command(Command::ConsentInsecure);
                self.screen = TabScreen::Connecting;
            }
            TabMessage::Close => {
                self.command(Command::Disconnect);
                return Outcome::Close;
            }
            TabMessage::ToggleMenu => self.menu_open = !self.menu_open,
            TabMessage::ToggleFullscreen => {
                self.menu_open = false;
                return Outcome::Task(self.set_full_screen(!self.full_screen, window));
            }
            TabMessage::ToggleViewOnly => {
                self.view_only = !self.view_only;
                self.command(Command::SetViewOnly(self.view_only));
            }
            TabMessage::SendCtrlAltDel => {
                self.menu_open = false;
                for keysym in CTRL_ALT_DEL {
                    self.command(Command::Key { down: true, keysym });
                }
                for keysym in CTRL_ALT_DEL.iter().rev() {
                    self.command(Command::Key {
                        down: false,
                        keysym: *keysym,
                    });
                }
            }
            TabMessage::MoveToWindow => {
                self.menu_open = false;
                return Outcome::Detach;
            }
            TabMessage::PointerMoved { remote, local } => {
                self.pointer = remote;
                self.local_pointer = Some(local);
                self.send_pointer();
            }
        }
        Outcome::None
    }

    fn submit_credentials(&mut self) {
        let username = (!self.username_input.is_empty())
            .then(|| SecretString::from(self.username_input.clone()));
        let password = SecretString::from(std::mem::take(&mut self.password_input));
        if self.remember {
            self.pending_remember = Some(Remember {
                username: username.clone(),
                password: password.clone(),
            });
        }
        self.command(Command::Credentials { username, password });
        self.screen = TabScreen::Connecting;
    }

    pub fn on_net(&mut self, net: Net, store: &mut Store) -> Outcome {
        match net {
            Net::Ready(handle) => self.handle = Some(handle),
            Net::Failed(reason) | Net::Update(Update::Closed(Some(reason))) => {
                self.handle = None;
                self.connected = None;
                self.screen = TabScreen::Failed(reason);
            }
            Net::Update(Update::Closed(None)) => return Outcome::Close,
            Net::Update(Update::Frame(frame)) => {
                self.frame = Some((
                    image::Handle::from_rgba(frame.width, frame.height, frame.rgba),
                    frame.width,
                    frame.height,
                ));
            }
            Net::Update(Update::Cursor(cursor)) => {
                self.cursor = cursor.map(|c| {
                    (
                        image::Handle::from_rgba(c.width, c.height, c.rgba.clone()),
                        c,
                    )
                });
            }
            Net::Update(Update::Event(event)) => return self.on_session_event(event, store),
        }
        Outcome::None
    }

    fn on_session_event(&mut self, event: SessionEvent, store: &mut Store) -> Outcome {
        match event {
            SessionEvent::CredentialsRequired { username } => {
                // Use remembered credentials once; ask if they were refused.
                if let Some(Remember { username, password }) = self.stored_credentials.take() {
                    self.command(Command::Credentials { username, password });
                } else {
                    self.screen = TabScreen::Credentials { username };
                }
            }
            SessionEvent::IdentityConfirmationRequired {
                fingerprint,
                kind,
                changed,
            } => {
                self.change_acknowledged = false;
                self.screen = TabScreen::Identity {
                    fingerprint,
                    kind,
                    changed,
                };
            }
            SessionEvent::InsecureConsentRequired { security } => {
                self.screen = TabScreen::InsecureWarning(security);
            }
            SessionEvent::Connected {
                name,
                security,
                encrypted,
                identity,
                ..
            } => {
                store.remember(&self.address, &name, identity, self.pending_remember.take());
                self.connected = Some(Connected {
                    name,
                    security,
                    encrypted,
                });
                self.screen = TabScreen::Desktop;
            }
            SessionEvent::Clipboard(text) => return Outcome::Task(iced::clipboard::write(text)),
            SessionEvent::TransportUpgrade(_)
            | SessionEvent::Damaged(_)
            | SessionEvent::Resized { .. }
            | SessionEvent::UpdateComplete
            | SessionEvent::CursorChanged
            | SessionEvent::Bell => {}
        }
        Outcome::None
    }

    pub fn key(
        &mut self,
        pressed: bool,
        key: &Key,
        modified: &Key,
        physical: Physical,
        location: Location,
    ) {
        if !self.is_desktop() {
            return;
        }
        if *key == Key::Named(Named::F8) && self.menu_key_f8 {
            if pressed {
                self.menu_open = !self.menu_open;
            }
            return;
        }
        if pressed {
            if let Some(keysym) = keymap::keysym(key, modified, location) {
                self.pressed_keys.insert(physical, keysym);
                self.command(Command::Key { down: true, keysym });
            }
        } else if let Some(keysym) = self.pressed_keys.remove(&physical) {
            // Release with the keysym we pressed, even if modifiers changed since.
            self.command(Command::Key {
                down: false,
                keysym,
            });
        }
    }

    /// Releases every key still held (when focus moves to another tab).
    pub fn release_keys(&mut self) {
        for (_, keysym) in self.pressed_keys.drain() {
            if let Some(handle) = &self.handle {
                let _ = handle.send(Command::Key {
                    down: false,
                    keysym,
                });
            }
        }
    }

    pub fn button(&mut self, pressed: bool, button: mouse::Button) {
        if !self.is_desktop() {
            return;
        }
        let bit = match button {
            mouse::Button::Left => 0,
            mouse::Button::Middle => 1,
            mouse::Button::Right => 2,
            // Provisional P-2: back as button 8 until ExtendedMouseButtons (M9).
            mouse::Button::Back => 7,
            mouse::Button::Forward | mouse::Button::Other(_) => return,
        };
        if pressed {
            self.buttons |= 1 << bit;
        } else {
            self.buttons &= !(1 << bit);
        }
        self.send_pointer();
    }

    pub fn scroll(&mut self, delta: mouse::ScrollDelta) {
        if !self.is_desktop() {
            return;
        }
        let (dx, dy) = match delta {
            mouse::ScrollDelta::Lines { x, y } => (x, y),
            mouse::ScrollDelta::Pixels { x, y } => (x / PIXELS_PER_NOTCH, y / PIXELS_PER_NOTCH),
        };
        self.scroll_accumulator.0 += dx;
        self.scroll_accumulator.1 += dy;
        // Buttons 4/5 scroll up/down, 6/7 scroll left/right.
        while self.scroll_accumulator.1 >= 1.0 {
            self.scroll_accumulator.1 -= 1.0;
            self.click(3);
        }
        while self.scroll_accumulator.1 <= -1.0 {
            self.scroll_accumulator.1 += 1.0;
            self.click(4);
        }
        while self.scroll_accumulator.0 >= 1.0 {
            self.scroll_accumulator.0 -= 1.0;
            self.click(5);
        }
        while self.scroll_accumulator.0 <= -1.0 {
            self.scroll_accumulator.0 += 1.0;
            self.click(6);
        }
    }

    fn click(&mut self, bit: u8) {
        let (x, y) = self.pointer;
        self.command(Command::Pointer {
            buttons: self.buttons | (1 << bit),
            x,
            y,
        });
        self.command(Command::Pointer {
            buttons: self.buttons,
            x,
            y,
        });
    }

    fn send_pointer(&self) {
        let (x, y) = self.pointer;
        self.command(Command::Pointer {
            buttons: self.buttons,
            x,
            y,
        });
    }

    pub fn set_full_screen(
        &mut self,
        full_screen: bool,
        window: Option<window::Id>,
    ) -> Task<Message> {
        self.full_screen = full_screen;
        let Some(id) = window else {
            return Task::none();
        };
        let mode = if full_screen {
            window::Mode::Fullscreen
        } else {
            window::Mode::Windowed
        };
        window::set_mode(id, mode)
    }

    fn message(&self, message: TabMessage) -> Message {
        Message::Tab(self.id, message)
    }

    /// The tab's content. `standalone` adds the status row a detached window
    /// needs (it has no header bar).
    pub fn view(&self, standalone: bool) -> Element<'_, Message> {
        let content: Element<'_, Message> = match &self.screen {
            TabScreen::Connecting => centred(
                widget::Column::new()
                    .push(widget::text::title3(format!(
                        "Connecting to {}…",
                        self.address.display_name()
                    )))
                    .push(
                        widget::button::standard("Cancel")
                            .on_press(self.message(TabMessage::Close)),
                    )
                    .spacing(12)
                    .align_x(Alignment::Center),
            ),
            TabScreen::Credentials { username } => self.credentials_view(*username),
            TabScreen::Identity {
                fingerprint,
                kind,
                changed,
            } => self.identity_view(fingerprint, *kind, *changed),
            TabScreen::InsecureWarning(security) => self.insecure_view(*security),
            TabScreen::Failed(reason) => centred(
                widget::Column::new()
                    .push(widget::text::title3("Couldn't connect"))
                    .push(widget::text::body(reason.clone()))
                    .push(
                        widget::button::suggested("Close")
                            .on_press(self.message(TabMessage::Close)),
                    )
                    .spacing(12)
                    .align_x(Alignment::Center),
            ),
            TabScreen::Desktop => self.desktop_view(),
        };
        let mut column = widget::Column::new();
        if standalone && !self.full_screen {
            column = column.push(self.status_row());
        }
        if self.menu_open && self.is_desktop() {
            column = column.push(self.menu_view());
        }
        column.push(content).into()
    }

    /// Encryption badge, view-only label and menu button.
    pub fn status_row(&self) -> Element<'_, Message> {
        let mut row = widget::Row::new()
            .spacing(8)
            .padding(4)
            .align_y(Alignment::Center);
        if let Some(connected) = &self.connected {
            row = row.push(security_badge(connected));
            if self.view_only {
                row = row.push(widget::text::body("View only"));
            }
            row = row.push(
                widget::button::icon(widget::icon::from_name("open-menu-symbolic"))
                    .tooltip("Viewer menu (F8)")
                    .on_press(self.message(TabMessage::ToggleMenu)),
            );
        }
        row.into()
    }

    fn credentials_view(&self, username: bool) -> Element<'_, Message> {
        let mut column = widget::Column::new()
            .push(widget::text::title3(format!(
                "Sign in to {}",
                self.address.display_name()
            )))
            .spacing(12)
            .align_x(Alignment::Center);
        if username {
            column = column.push(
                widget::text_input("Username", &self.username_input)
                    .on_input(|t| self.message(TabMessage::UsernameChanged(t)))
                    .width(Length::Fixed(320.0)),
            );
        }
        centred(
            column
                .push(
                    widget::secure_input(
                        "Password",
                        &self.password_input,
                        Some(self.message(TabMessage::TogglePasswordVisibility)),
                        self.password_hidden,
                    )
                    .on_input(|t| self.message(TabMessage::PasswordChanged(t)))
                    .on_submit(|_| self.message(TabMessage::SubmitCredentials))
                    .width(Length::Fixed(320.0)),
                )
                .push(
                    widget::checkbox(self.remember)
                        .label("Remember on this computer")
                        .on_toggle(|r| self.message(TabMessage::ToggleRemember(r))),
                )
                .push(
                    widget::Row::new()
                        .push(
                            widget::button::standard("Cancel")
                                .on_press(self.message(TabMessage::Close)),
                        )
                        .push(
                            widget::button::suggested("Connect")
                                .on_press(self.message(TabMessage::SubmitCredentials)),
                        )
                        .spacing(8),
                ),
        )
    }

    fn identity_view(
        &self,
        fingerprint: &Fingerprint,
        kind: IdentityKind,
        changed: bool,
    ) -> Element<'_, Message> {
        let what = match kind {
            IdentityKind::Certificate => "certificate",
            IdentityKind::RsaKey => "key",
        };
        let name = self.address.display_name();
        let (title, explanation) = if changed {
            (
                "This computer's identity has changed",
                format!(
                    "The {what} that {name} presented is not the one you trusted before. This happens after a \
                     reinstall, but it is also exactly what an attacker intercepting the connection looks like. \
                     Nothing has been sent yet."
                ),
            )
        } else {
            (
                "Is this the right computer?",
                format!(
                    "This is the first encrypted connection to {name}. Check that the fingerprint below matches \
                     the one shown on that computer (`farsight-agent --show-identity`), then trust it. \
                     Farsight will remember it."
                ),
            )
        };
        let mut column = widget::Column::new()
            .push(
                widget::icon::from_name(if changed {
                    "dialog-warning-symbolic"
                } else {
                    "security-high-symbolic"
                })
                .size(48),
            )
            .push(widget::text::title3(title))
            .push(widget::text::body(explanation).width(Length::Fixed(520.0)))
            .push(widget::text::monotext(format_fingerprint(fingerprint)))
            .spacing(12)
            .align_x(Alignment::Center);
        if changed {
            // A changed identity takes a deliberate extra step (fingerprint fatigue).
            column = column.push(
                widget::checkbox(self.change_acknowledged)
                    .label("I have checked the new fingerprint with the computer's owner")
                    .on_toggle(|a| self.message(TabMessage::AcknowledgeChange(a))),
            );
            let trust = widget::button::destructive("Trust the new identity").on_press_maybe(
                self.change_acknowledged
                    .then(|| self.message(TabMessage::TrustIdentity)),
            );
            column = column.push(
                widget::Row::new()
                    .push(
                        widget::button::suggested("Disconnect")
                            .on_press(self.message(TabMessage::Close)),
                    )
                    .push(trust)
                    .spacing(8),
            );
        } else {
            column = column.push(
                widget::Row::new()
                    .push(
                        widget::button::standard("Cancel")
                            .on_press(self.message(TabMessage::Close)),
                    )
                    .push(
                        widget::button::suggested("Trust")
                            .on_press(self.message(TabMessage::TrustIdentity)),
                    )
                    .spacing(8),
            );
        }
        centred(column)
    }

    fn insecure_view(&self, security: SecurityType) -> Element<'_, Message> {
        let detail = match security {
            SecurityType::None => {
                "Anyone on the network can see and control this session, and no password is needed."
            }
            _ => {
                "Anyone on the network can see everything on this screen and what you type. The password check is weak."
            }
        };
        centred(
            widget::Column::new()
                .push(widget::icon::from_name("dialog-warning-symbolic").size(48))
                .push(widget::text::title3("This connection is not encrypted"))
                .push(widget::text::body(detail))
                .push(widget::text::body(
                    "Only continue on a network you trust, such as your home network.",
                ))
                .push(
                    widget::Row::new()
                        .push(
                            widget::button::suggested("Cancel")
                                .on_press(self.message(TabMessage::Close)),
                        )
                        .push(
                            widget::button::destructive("Connect anyway")
                                .on_press(self.message(TabMessage::AcceptInsecure)),
                        )
                        .spacing(8),
                )
                .spacing(12)
                .align_x(Alignment::Center),
        )
    }

    fn desktop_view(&self) -> Element<'_, Message> {
        let Some((handle, width, height)) = self.frame.clone() else {
            return centred(widget::text::body("Waiting for the first picture…"));
        };
        let cursor = self.cursor.clone();
        let local_pointer = self.local_pointer;
        let id = self.id;
        widget::responsive(move |size: Size| {
            let mapping = Mapping::new(size, width, height);
            let interaction = if cursor.is_some() {
                mouse::Interaction::Hidden
            } else {
                mouse::Interaction::Crosshair
            };
            let picture = widget::mouse_area(
                widget::image(handle.clone())
                    .content_fit(iced::ContentFit::Contain)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .interaction(interaction)
            .on_move(move |point| {
                Message::Tab(
                    id,
                    TabMessage::PointerMoved {
                        remote: mapping.to_remote(point),
                        local: point,
                    },
                )
            });
            let overlay = CursorOverlay {
                cursor: cursor.clone(),
                position: local_pointer,
                scale: mapping.scale,
            };
            iced::widget::stack![
                picture,
                widget::canvas(overlay)
                    .width(Length::Fill)
                    .height(Length::Fill)
            ]
            .into()
        })
        .into()
    }

    fn menu_view(&self) -> Element<'_, Message> {
        let mut row = widget::Row::new()
            .push(
                widget::button::standard(if self.full_screen {
                    "Leave full screen"
                } else {
                    "Full screen"
                })
                .on_press(self.message(TabMessage::ToggleFullscreen)),
            )
            .push(
                widget::button::standard(if self.view_only {
                    "Allow control"
                } else {
                    "View only"
                })
                .on_press(self.message(TabMessage::ToggleViewOnly)),
            )
            .push(
                widget::button::standard("Send Ctrl+Alt+Del")
                    .on_press(self.message(TabMessage::SendCtrlAltDel)),
            );
        if self.window.is_none() {
            row = row.push(
                widget::button::standard("Move to new window")
                    .on_press(self.message(TabMessage::MoveToWindow)),
            );
        }
        row = row
            .push(
                widget::button::destructive("Disconnect").on_press(self.message(TabMessage::Close)),
            )
            .push(
                widget::button::text("Close menu").on_press(self.message(TabMessage::ToggleMenu)),
            );
        if self.full_screen
            && let Some(connected) = &self.connected
        {
            row = row.push(security_badge(connected));
        }
        row.spacing(8).padding(8).into()
    }
}

pub fn security_badge(connected: &Connected) -> Element<'_, Message> {
    let (icon, label) = if connected.encrypted {
        ("security-high-symbolic", "Encrypted".to_owned())
    } else {
        (
            "security-low-symbolic",
            format!("Not encrypted ({:?})", connected.security),
        )
    };
    widget::Row::new()
        .push(widget::icon::from_name(icon).size(16))
        .push(widget::text::body(label))
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
}

/// Runs one connection and streams its progress to the app.
pub fn connection(
    request: &ConnectRequest,
) -> impl iced::futures::Stream<Item = (TabId, Net)> + use<> {
    let request = request.clone();
    iced::stream::channel(64, async move |mut output| {
        let tab = request.tab;
        let address = ServerAddress {
            host: request.host.clone(),
            port: request.port,
        };
        let config = SessionConfig {
            pinned_identity: request.pinned,
            expect_encryption: request.pinned.is_some(),
            view_only: request.view_only,
            shared: request.shared,
            quality: request.quality,
            compress: request.compress,
            allow_insecure: request.allow_insecure,
            ..SessionConfig::default()
        };
        match farsight_net::connect(&address, config).await {
            Ok((handle, mut updates)) => {
                let _ = output.send((tab, Net::Ready(handle))).await;
                while let Some(update) = updates.recv().await {
                    let closed = matches!(update, Update::Closed(_));
                    if output.send((tab, Net::Update(update))).await.is_err() || closed {
                        break;
                    }
                }
            }
            Err(error) => {
                let _ = output.send((tab, Net::Failed(error.to_string()))).await;
            }
        }
    })
}
