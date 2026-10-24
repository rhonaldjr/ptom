//! The viewer window: connection dialog, prompts, and the remote desktop.
//!
//! A humble object (ADR-0002): protocol and session logic live in
//! `farsight-client`; this module only renders state and forwards input.

use std::collections::HashMap;

use cosmic::app::{Core, Task};
use cosmic::iced::futures::SinkExt;
use cosmic::iced::keyboard::{self, Key, Location, key::Named, key::Physical};
use cosmic::iced::{Alignment, Length, Point, Size, Subscription, event, mouse, window};
use cosmic::prelude::*;
use cosmic::widget::{self, image};
use cosmic::{executor, iced};
use farsight_client::{ServerAddress, SessionConfig, SessionEvent, ViewerOptions};
use farsight_net::{Command, ConnectionHandle, CursorImage, Update};
use farsight_rfb::SecurityType;
use secrecy::SecretString;

use crate::keymap;

/// Keysyms sent by the "Send Ctrl+Alt+Del" menu entry.
const CTRL_ALT_DEL: [u32; 3] = [0xffe3, 0xffe9, 0xffff];
/// Approximate height of the COSMIC header bar, in logical pixels.
const HEADER_HEIGHT: f32 = 48.0;
/// Wheel distance, in pixels, that counts as one notch.
const PIXELS_PER_NOTCH: f32 = 50.0;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Screen {
    Connect,
    Connecting,
    Password,
    InsecureWarning(SecurityType),
    Desktop,
    Failed(String),
}

/// What identifies one connection attempt (drives the subscription).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct ConnectRequest {
    attempt: u64,
    host: String,
    port: u16,
    view_only: bool,
    shared: bool,
    quality: Option<u8>,
    compress: Option<u8>,
    allow_insecure: bool,
}

#[derive(Debug, Clone)]
pub enum Net {
    Ready(ConnectionHandle),
    Update(Update),
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    AddressChanged(String),
    Connect,
    BackToStart,
    PasswordChanged(String),
    TogglePasswordVisibility,
    SubmitPassword,
    AcceptInsecure,
    Net(Net),
    Key {
        pressed: bool,
        key: Key,
        modified: Key,
        physical: Physical,
        location: Location,
    },
    PointerMoved {
        remote: (u16, u16),
        local: Point,
    },
    Button {
        pressed: bool,
        button: mouse::Button,
    },
    Scroll(mouse::ScrollDelta),
    WindowFocused,
    WindowResized(Size),
    ResizeTick,
    LocalClipboard(Option<String>),
    ToggleMenu,
    ToggleFullscreen,
    ToggleViewOnly,
    SendCtrlAltDel,
    Disconnect,
}

#[derive(Debug, Clone)]
struct Connected {
    name: String,
    security: SecurityType,
    encrypted: bool,
}

#[allow(clippy::struct_excessive_bools)] // Independent UI toggles.
pub struct App {
    core: Core,
    options: ViewerOptions,
    screen: Screen,
    address_input: String,
    password_input: String,
    password_hidden: bool,
    request: Option<ConnectRequest>,
    attempts: u64,
    handle: Option<ConnectionHandle>,
    connected: Option<Connected>,
    frame: Option<(image::Handle, u32, u32)>,
    cursor: Option<(image::Handle, CursorImage)>,
    local_pointer: Option<Point>,
    pending_resize: Option<(u16, u16)>,
    view_only: bool,
    full_screen: bool,
    menu_open: bool,
    buttons: u8,
    pointer: (u16, u16),
    scroll_accumulator: (f32, f32),
    pressed_keys: HashMap<Physical, u32>,
}

impl cosmic::Application for App {
    type Executor = executor::Default;
    type Flags = ViewerOptions;
    type Message = Message;

    const APP_ID: &'static str = "ca.akrotechnologies.Farsight";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, options: ViewerOptions) -> (Self, Task<Message>) {
        let mut app = Self {
            core,
            address_input: options
                .server
                .as_ref()
                .map(ServerAddress::display_name)
                .unwrap_or_default(),
            view_only: options.view_only,
            full_screen: options.full_screen,
            options,
            screen: Screen::Connect,
            password_input: String::new(),
            password_hidden: true,
            request: None,
            attempts: 0,
            handle: None,
            connected: None,
            frame: None,
            cursor: None,
            local_pointer: None,
            pending_resize: None,
            menu_open: false,
            buttons: 0,
            pointer: (0, 0),
            scroll_accumulator: (0.0, 0.0),
            pressed_keys: HashMap::new(),
        };
        let mut tasks = vec![app.update_title()];
        if let Some(server) = app.options.server.clone() {
            tasks.push(app.start(server, app.options.allow_insecure));
        }
        (app, Task::batch(tasks))
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![event::listen_with(on_event)];
        if self.pending_resize.is_some() {
            subscriptions.push(
                iced::time::every(std::time::Duration::from_millis(500))
                    .map(|_| Message::ResizeTick),
            );
        }
        if let Some(request) = &self.request {
            subscriptions
                .push(Subscription::run_with(request.clone(), connection).map(Message::Net));
        }
        Subscription::batch(subscriptions)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::AddressChanged(text) => self.address_input = text,
            Message::Connect => match ServerAddress::parse(&self.address_input) {
                Ok(address) => return self.start(address, self.options.allow_insecure),
                Err(e) => self.screen = Screen::Failed(e.to_string()),
            },
            Message::BackToStart | Message::Disconnect => return self.disconnect(),
            Message::PasswordChanged(text) => self.password_input = text,
            Message::TogglePasswordVisibility => self.password_hidden = !self.password_hidden,
            Message::SubmitPassword => {
                let password = SecretString::from(std::mem::take(&mut self.password_input));
                self.command(Command::Password(password));
                self.screen = Screen::Connecting;
            }
            Message::AcceptInsecure => {
                self.command(Command::ConsentInsecure);
                self.screen = Screen::Connecting;
            }
            Message::Net(net) => return self.on_net(net),
            Message::Key {
                pressed,
                key,
                modified,
                physical,
                location,
            } => {
                return self.on_key(pressed, &key, &modified, physical, location);
            }
            Message::PointerMoved { remote, local } => {
                self.pointer = remote;
                self.local_pointer = Some(local);
                self.send_pointer();
            }
            Message::Button { pressed, button } => self.on_button(pressed, button),
            Message::Scroll(delta) => self.on_scroll(delta),
            Message::WindowFocused => {
                if self.screen == Screen::Desktop {
                    return iced::clipboard::read()
                        .map(|text| cosmic::Action::App(Message::LocalClipboard(text)));
                }
            }
            Message::WindowResized(size) => {
                if self.options.remote_resize {
                    // The header bar takes part of the window unless full screen.
                    let header = if self.full_screen { 0.0 } else { HEADER_HEIGHT };
                    self.pending_resize = Some((to_u16(size.width), to_u16(size.height - header)));
                }
            }
            Message::ResizeTick => {
                if let Some((width, height)) = self.pending_resize.take() {
                    self.command(Command::Resize { width, height });
                }
            }
            Message::LocalClipboard(Some(text)) => self.command(Command::Clipboard(text)),
            Message::LocalClipboard(None) => {}
            Message::ToggleMenu => self.menu_open = !self.menu_open,
            Message::ToggleFullscreen => {
                self.menu_open = false;
                return self.set_full_screen(!self.full_screen);
            }
            Message::ToggleViewOnly => {
                self.view_only = !self.view_only;
                self.command(Command::SetViewOnly(self.view_only));
            }
            Message::SendCtrlAltDel => {
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
        }
        Task::none()
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        let mut items = Vec::new();
        if let Some(connected) = &self.connected {
            items.push(security_badge(connected));
            if self.view_only {
                items.push(widget::text::body("View only").into());
            }
            items.push(
                widget::button::icon(widget::icon::from_name("open-menu-symbolic"))
                    .tooltip("Viewer menu (F8)")
                    .on_press(Message::ToggleMenu)
                    .into(),
            );
        }
        items
    }

    fn view(&self) -> Element<'_, Message> {
        let content: Element<'_, Message> = match &self.screen {
            Screen::Connect => self.connect_view(),
            Screen::Connecting => centred(
                widget::Column::new()
                    .push(widget::text::title3("Connecting…"))
                    .push(widget::button::standard("Cancel").on_press(Message::BackToStart))
                    .spacing(12)
                    .align_x(Alignment::Center),
            ),
            Screen::Password => self.password_view(),
            Screen::InsecureWarning(security) => insecure_view(*security),
            Screen::Failed(reason) => centred(
                widget::Column::new()
                    .push(widget::text::title3("Couldn't connect"))
                    .push(widget::text::body(reason.clone()))
                    .push(widget::button::suggested("Back").on_press(Message::BackToStart))
                    .spacing(12)
                    .align_x(Alignment::Center),
            ),
            Screen::Desktop => self.desktop_view(),
        };
        if self.menu_open && self.screen == Screen::Desktop {
            widget::Column::new()
                .push(self.menu_view())
                .push(content)
                .into()
        } else {
            content
        }
    }
}

impl App {
    fn update_title(&mut self) -> Task<Message> {
        let title = match &self.connected {
            Some(c) => format!("{} — Farsight", c.name),
            None => "Farsight".to_owned(),
        };
        self.set_header_title(title.clone());
        match self.core.main_window_id() {
            Some(id) => self.set_window_title(title, id),
            None => Task::none(),
        }
    }

    fn start(&mut self, address: ServerAddress, allow_insecure: bool) -> Task<Message> {
        self.attempts += 1;
        self.address_input = address.display_name();
        self.request = Some(ConnectRequest {
            attempt: self.attempts,
            host: address.host,
            port: address.port,
            view_only: self.view_only,
            shared: self.options.shared,
            quality: self.options.quality,
            compress: self.options.compress,
            allow_insecure,
        });
        self.screen = Screen::Connecting;
        Task::none()
    }

    fn disconnect(&mut self) -> Task<Message> {
        self.command(Command::Disconnect);
        self.request = None;
        self.handle = None;
        self.connected = None;
        self.frame = None;
        self.cursor = None;
        self.local_pointer = None;
        self.menu_open = false;
        self.screen = Screen::Connect;
        let title = self.update_title();
        Task::batch([title, self.set_full_screen(false)])
    }

    fn command(&self, command: Command) {
        if let Some(handle) = &self.handle {
            // A closed connection reports itself through `Update::Closed`.
            let _ = handle.send(command);
        }
    }

    fn on_net(&mut self, net: Net) -> Task<Message> {
        match net {
            Net::Ready(handle) => self.handle = Some(handle),
            Net::Failed(reason) | Net::Update(Update::Closed(Some(reason))) => {
                self.request = None;
                self.handle = None;
                self.connected = None;
                self.screen = Screen::Failed(reason);
                return self.update_title();
            }
            Net::Update(Update::Closed(None)) => return self.disconnect(),
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
            Net::Update(Update::Event(event)) => return self.on_session_event(event),
        }
        Task::none()
    }

    fn on_session_event(&mut self, event: SessionEvent) -> Task<Message> {
        match event {
            SessionEvent::PasswordRequired => self.screen = Screen::Password,
            SessionEvent::InsecureConsentRequired { security } => {
                self.screen = Screen::InsecureWarning(security);
            }
            SessionEvent::Connected {
                name,
                security,
                encrypted,
                ..
            } => {
                self.connected = Some(Connected {
                    name,
                    security,
                    encrypted,
                });
                self.screen = Screen::Desktop;
                let full_screen = self.set_full_screen(self.full_screen);
                return Task::batch([self.update_title(), full_screen]);
            }
            SessionEvent::Clipboard(text) => return iced::clipboard::write(text),
            SessionEvent::Damaged(_)
            | SessionEvent::Resized { .. }
            | SessionEvent::UpdateComplete
            | SessionEvent::CursorChanged
            | SessionEvent::Bell => {}
        }
        Task::none()
    }

    fn on_key(
        &mut self,
        pressed: bool,
        key: &Key,
        modified: &Key,
        physical: Physical,
        location: Location,
    ) -> Task<Message> {
        if self.screen != Screen::Desktop {
            return Task::none();
        }
        if *key == Key::Named(Named::F8) && self.options.menu_key.eq_ignore_ascii_case("F8") {
            if pressed {
                self.menu_open = !self.menu_open;
            }
            return Task::none();
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
        Task::none()
    }

    fn on_button(&mut self, pressed: bool, button: mouse::Button) {
        if self.screen != Screen::Desktop {
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

    fn on_scroll(&mut self, delta: mouse::ScrollDelta) {
        if self.screen != Screen::Desktop {
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

    fn set_full_screen(&mut self, full_screen: bool) -> Task<Message> {
        self.full_screen = full_screen;
        self.core.window.show_headerbar = !full_screen;
        let Some(id) = self.core.main_window_id() else {
            return Task::none();
        };
        let mode = if full_screen {
            window::Mode::Fullscreen
        } else {
            window::Mode::Windowed
        };
        window::set_mode(id, mode)
    }

    fn connect_view(&self) -> Element<'_, Message> {
        centred(
            widget::Column::new()
                .push(widget::text::title2("Connect to a computer"))
                .push(widget::text::body(
                    "Enter its name or address, for example pop-os:1 or 192.168.1.20",
                ))
                .push(
                    widget::text_input("Computer", &self.address_input)
                        .on_input(Message::AddressChanged)
                        .on_submit(|_| Message::Connect)
                        .width(Length::Fixed(360.0)),
                )
                .push(widget::button::suggested("Connect").on_press(Message::Connect))
                .spacing(12)
                .align_x(Alignment::Center),
        )
    }

    fn password_view(&self) -> Element<'_, Message> {
        centred(
            widget::Column::new()
                .push(widget::text::title3(format!(
                    "Password for {}",
                    self.address_input
                )))
                .push(
                    widget::secure_input(
                        "Password",
                        &self.password_input,
                        Some(Message::TogglePasswordVisibility),
                        self.password_hidden,
                    )
                    .on_input(Message::PasswordChanged)
                    .on_submit(|_| Message::SubmitPassword)
                    .width(Length::Fixed(320.0)),
                )
                .push(
                    widget::Row::new()
                        .push(widget::button::standard("Cancel").on_press(Message::BackToStart))
                        .push(
                            widget::button::suggested("Connect").on_press(Message::SubmitPassword),
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
            .on_move(move |point| Message::PointerMoved {
                remote: mapping.to_remote(point),
                local: point,
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
        widget::Row::new()
            .push(
                widget::button::standard(if self.full_screen {
                    "Leave full screen"
                } else {
                    "Full screen"
                })
                .on_press(Message::ToggleFullscreen),
            )
            .push(
                widget::button::standard(if self.view_only {
                    "Allow control"
                } else {
                    "View only"
                })
                .on_press(Message::ToggleViewOnly),
            )
            .push(widget::button::standard("Send Ctrl+Alt+Del").on_press(Message::SendCtrlAltDel))
            .push(widget::button::destructive("Disconnect").on_press(Message::Disconnect))
            .push(widget::button::text("Close menu").on_press(Message::ToggleMenu))
            .push_maybe(self.full_screen.then(|| -> Element<'_, Message> {
                self.connected
                    .as_ref()
                    .map_or_else(|| widget::text::body("").into(), security_badge)
            }))
            .spacing(8)
            .padding(8)
            .into()
    }
}

/// Draws the remote cursor shape at the local pointer position.
struct CursorOverlay {
    cursor: Option<(image::Handle, CursorImage)>,
    position: Option<Point>,
    scale: f32,
}

impl iced::widget::canvas::Program<Message, cosmic::Theme> for CursorOverlay {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &cosmic::Renderer,
        _theme: &cosmic::Theme,
        bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<iced::widget::canvas::Geometry> {
        let mut frame = iced::widget::canvas::Frame::new(renderer, bounds.size());
        if let (Some((handle, cursor)), Some(position)) = (&self.cursor, self.position) {
            #[allow(clippy::cast_precision_loss)] // Cursors are at most 256 pixels.
            let (w, h, hx, hy) = (
                cursor.width as f32 * self.scale,
                cursor.height as f32 * self.scale,
                f32::from(cursor.hotspot.0) * self.scale,
                f32::from(cursor.hotspot.1) * self.scale,
            );
            let area = iced::Rectangle::new(
                Point::new(position.x - hx, position.y - hy),
                Size::new(w, h),
            );
            frame.draw_image(area, iced::widget::canvas::Image::new(handle.clone()));
        }
        vec![frame.into_geometry()]
    }
}

/// Maps widget coordinates to framebuffer coordinates for a letterboxed image.
#[derive(Debug, Clone, Copy)]
struct Mapping {
    scale: f32,
    offset_x: f32,
    offset_y: f32,
    width: u32,
    height: u32,
}

impl Mapping {
    fn new(size: Size, width: u32, height: u32) -> Self {
        #[allow(clippy::cast_precision_loss)] // Screen sizes are far below 2^24.
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let scale = (size.width / w).min(size.height / h);
        Self {
            scale,
            offset_x: (size.width - w * scale) / 2.0,
            offset_y: (size.height - h * scale) / 2.0,
            width,
            height,
        }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Clamped to 0..=u16::MAX first.
    fn to_remote(self, point: Point) -> (u16, u16) {
        let clamp = |v: f32, max: u32| -> u16 {
            #[allow(clippy::cast_precision_loss)]
            let max = max.saturating_sub(1).min(u32::from(u16::MAX)) as f32;
            v.clamp(0.0, max) as u16
        };
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        (
            clamp((point.x - self.offset_x) / scale, self.width),
            clamp((point.y - self.offset_y) / scale, self.height),
        )
    }
}

fn security_badge(connected: &Connected) -> Element<'_, Message> {
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

fn insecure_view(security: SecurityType) -> Element<'static, Message> {
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
                    .push(widget::button::suggested("Cancel").on_press(Message::BackToStart))
                    .push(
                        widget::button::destructive("Connect anyway")
                            .on_press(Message::AcceptInsecure),
                    )
                    .spacing(8),
            )
            .spacing(12)
            .align_x(Alignment::Center),
    )
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Clamped to the u16 range first.
fn to_u16(value: f32) -> u16 {
    value.clamp(0.0, f32::from(u16::MAX)) as u16
}

fn centred<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    widget::container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .padding(24)
        .into()
}

/// Turns window events into messages. Keys already handled by a widget
/// (e.g. typed into a text field) are not forwarded.
fn on_event(event: event::Event, status: event::Status, _window: window::Id) -> Option<Message> {
    match event {
        event::Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            modified_key,
            physical_key,
            location,
            ..
        }) if status == event::Status::Ignored => Some(Message::Key {
            pressed: true,
            key,
            modified: modified_key,
            physical: physical_key,
            location,
        }),
        event::Event::Keyboard(keyboard::Event::KeyReleased {
            key,
            modified_key,
            physical_key,
            location,
            ..
        }) => Some(Message::Key {
            pressed: false,
            key,
            modified: modified_key,
            physical: physical_key,
            location,
        }),
        event::Event::Mouse(mouse::Event::ButtonPressed(button))
            if status == event::Status::Ignored =>
        {
            Some(Message::Button {
                pressed: true,
                button,
            })
        }
        event::Event::Mouse(mouse::Event::ButtonReleased(button)) => Some(Message::Button {
            pressed: false,
            button,
        }),
        event::Event::Mouse(mouse::Event::WheelScrolled { delta })
            if status == event::Status::Ignored =>
        {
            Some(Message::Scroll(delta))
        }
        event::Event::Window(window::Event::Focused) => Some(Message::WindowFocused),
        event::Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
        _ => None,
    }
}

/// Runs one connection and streams its progress to the app.
fn connection(request: &ConnectRequest) -> impl iced::futures::Stream<Item = Net> + use<> {
    let request = request.clone();
    iced::stream::channel(64, async move |mut output| {
        let address = ServerAddress {
            host: request.host.clone(),
            port: request.port,
        };
        let config = SessionConfig {
            view_only: request.view_only,
            shared: request.shared,
            quality: request.quality,
            compress: request.compress,
            allow_insecure: request.allow_insecure,
            ..SessionConfig::default()
        };
        match farsight_net::connect(&address, config).await {
            Ok((handle, mut updates)) => {
                let _ = output.send(Net::Ready(handle)).await;
                while let Some(update) = updates.recv().await {
                    let closed = matches!(update, Update::Closed(_));
                    if output.send(Net::Update(update)).await.is_err() || closed {
                        break;
                    }
                }
            }
            Err(error) => {
                let _ = output.send(Net::Failed(error.to_string())).await;
            }
        }
    })
}
