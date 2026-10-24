//! The viewer shell (M6): the Home tab (unlock, connect, computers on your
//! network), one tab per connection, tabs moved to their own windows, and the
//! tray icon.
//!
//! A humble object (ADR-0002): protocol and session logic live in
//! `farsight-client`; tabs hold per-connection UI state (`tab.rs`).

use std::sync::{Arc, Mutex, PoisonError};

use cosmic::app::{Core, Task};
use cosmic::iced::keyboard::{self, Key, Location, key::Physical};
use cosmic::iced::{Alignment, Length, Size, Subscription, event, mouse, window};
use cosmic::prelude::*;
use cosmic::widget;
use cosmic::{executor, iced};
use farsight_client::{ServerAddress, ViewerOptions};
use farsight_desktop::tray::{TrayAction, TrayAvailability, TrayEntry, TrayModel, ViewerTray};
use secrecy::SecretString;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::nearby::{Discovery, Encryption, Nearby, NearbyEntry, Scan};
use crate::store::{Entry, Remember, Store};
use crate::tab::{ConnectRequest, Net, Outcome, Tab, TabId, TabMessage, connection};
use crate::views::{centred, to_u16};

/// Approximate height of the COSMIC header bar, in logical pixels.
const HEADER_HEIGHT: f32 = 48.0;
/// Approximate height of the tab bar.
const TAB_BAR_HEIGHT: f32 = 40.0;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Home {
    Unlock {
        creating: bool,
        error: Option<String>,
    },
    Connect,
    Failed(String),
}

/// The running tray icon (cheap to clone into messages).
#[derive(Clone)]
pub struct TrayHandle(Arc<ViewerTray>);

impl std::fmt::Debug for TrayHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TrayHandle")
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    AddressChanged(String),
    Connect,
    Pick(ServerAddress),
    Nearby(NearbyMessage),
    PassphraseChanged(String),
    Unlock,
    SkipUnlock,
    ShowHome,
    Activate(TabId),
    CloseTab(TabId),
    Tab(TabId, TabMessage),
    Net(TabId, Net),
    Key {
        window: window::Id,
        pressed: bool,
        key: Key,
        modified: Key,
        physical: Physical,
        location: Location,
    },
    Button {
        window: window::Id,
        pressed: bool,
        button: mouse::Button,
    },
    Scroll {
        window: window::Id,
        delta: mouse::ScrollDelta,
    },
    WindowFocused(window::Id),
    WindowResized(window::Id, Size),
    WindowClosed(window::Id),
    ResizeTick,
    LocalClipboard(window::Id, Option<String>),
    TrayStarted(TrayHandle, TrayAvailability),
    Tray(TrayAction),
    Noop,
}

/// Messages for "Computers on your network".
#[derive(Debug, Clone)]
pub enum NearbyMessage {
    Discovery(Discovery),
    TrustNetworkAndScan,
    Rescan,
    CancelScan,
    ForgetNetwork,
    Add(ServerAddress, String),
}

/// Tray picks waiting for the subscription to take them (set once at start).
static TRAY_ACTIONS: Mutex<Option<UnboundedReceiver<TrayAction>>> = Mutex::new(None);

pub struct App {
    core: Core,
    options: ViewerOptions,
    home: Home,
    address_input: String,
    passphrase_input: String,
    store: Store,
    nearby: Nearby,
    network: Option<String>,
    scan_attempt: Option<u64>,
    counter: u64,
    tabs: Vec<Tab>,
    /// The tab shown in the main window; `None` shows Home.
    active: Option<TabId>,
    tray: Option<TrayHandle>,
    tray_model: TrayModel,
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
            address_input: String::new(),
            options,
            home: Home::Connect,
            passphrase_input: String::new(),
            store: Store::open(),
            nearby: Nearby::default(),
            network: farsight_discovery::current_networks()
                .first()
                .map(|n| n.as_str().to_owned()),
            scan_attempt: None,
            counter: 0,
            tabs: Vec::new(),
            active: None,
            tray: None,
            tray_model: TrayModel::default(),
        };
        let mut tasks = vec![app.update_title(), app.start_tray()];
        app.scan_if_trusted();
        if app.store.locked() {
            app.home = Home::Unlock {
                creating: !Store::exists(),
                error: None,
            };
        } else if let Some(server) = app.options.server.clone() {
            tasks.push(app.open_tab(server, app.options.allow_insecure));
        }
        (app, Task::batch(tasks))
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            event::listen_with(on_event),
            window::close_events().map(Message::WindowClosed),
            Subscription::run(crate::nearby::browse)
                .map(|d| Message::Nearby(NearbyMessage::Discovery(d))),
            Subscription::run(tray_actions).map(Message::Tray),
        ];
        if self.tabs.iter().any(|t| t.pending_resize.is_some()) {
            subscriptions.push(
                iced::time::every(std::time::Duration::from_millis(500))
                    .map(|_| Message::ResizeTick),
            );
        }
        if let Some(attempt) = self.scan_attempt {
            subscriptions.push(
                Subscription::run_with(attempt, crate::nearby::scan)
                    .map(|d| Message::Nearby(NearbyMessage::Discovery(d))),
            );
        }
        for tab in &self.tabs {
            subscriptions.push(
                Subscription::run_with(tab.request.clone(), connection)
                    .map(|(id, net)| Message::Net(id, net)),
            );
        }
        Subscription::batch(subscriptions)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        Task::batch([task, self.sync_tray()])
    }

    fn header_end(&self) -> Vec<Element<'_, Message>> {
        match self.active_tab() {
            Some(tab) => vec![tab.status_row()],
            None => Vec::new(),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let content = match self.active_tab() {
            Some(tab) => tab.view(false),
            None => self.home_view(),
        };
        let main_tabs: Vec<&Tab> = self.tabs.iter().filter(|t| t.window.is_none()).collect();
        let full_screen = self.active_tab().is_some_and(|t| t.full_screen);
        if main_tabs.is_empty() || full_screen {
            return content;
        }
        widget::Column::new()
            .push(self.tab_bar(&main_tabs))
            .push(content)
            .into()
    }

    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        match self.tabs.iter().find(|t| t.window == Some(id)) {
            Some(tab) => tab.view(true),
            None => centred(widget::text::body("This session has ended.")),
        }
    }
}

impl App {
    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::AddressChanged(text) => self.address_input = text,
            Message::Connect => match ServerAddress::parse(&self.address_input) {
                Ok(address) => return self.open_tab(address, self.options.allow_insecure),
                Err(e) => self.home = Home::Failed(e.to_string()),
            },
            Message::Pick(address) => return self.open_tab(address, self.options.allow_insecure),
            Message::Nearby(message) => self.on_nearby(message),
            Message::PassphraseChanged(text) => self.passphrase_input = text,
            Message::Unlock => {
                let passphrase = SecretString::from(std::mem::take(&mut self.passphrase_input));
                match self.store.unlock(passphrase) {
                    Ok(()) => return self.after_unlock(),
                    Err(error) => {
                        self.home = Home::Unlock {
                            creating: !Store::exists(),
                            error: Some(error),
                        }
                    }
                }
            }
            Message::SkipUnlock => {
                self.store.skip();
                return self.after_unlock();
            }
            Message::ShowHome => {
                self.home = Home::Connect;
                return self.activate(None);
            }
            Message::Activate(id) => return self.activate(Some(id)),
            Message::CloseTab(id) => return self.close_tab(id),
            Message::Tab(id, message) => return self.on_tab_message(id, message),
            Message::Net(id, net) => {
                let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else {
                    return Task::none();
                };
                let outcome = tab.on_net(net, &mut self.store);
                return self.apply(id, outcome);
            }
            Message::Key { .. }
            | Message::Button { .. }
            | Message::Scroll { .. }
            | Message::WindowFocused(_)
            | Message::LocalClipboard(..)
            | Message::WindowResized(..)
            | Message::ResizeTick => return self.on_input(message),
            Message::Noop => {}
            Message::WindowClosed(window) => {
                if let Some(id) = self
                    .tabs
                    .iter()
                    .find(|t| t.window == Some(window))
                    .map(|t| t.id)
                {
                    if let Some(tab) = self.tab_mut(id) {
                        tab.window = None;
                        tab.command(farsight_net::Command::Disconnect);
                    }
                    self.tabs.retain(|t| t.id != id);
                }
            }
            Message::TrayStarted(handle, availability) => {
                if availability == TrayAvailability::Available {
                    self.tray = Some(handle);
                    self.tray_model = TrayModel::default();
                }
            }
            Message::Tray(action) => return self.on_tray(action),
        }
        Task::none()
    }

    /// Keyboard, pointer, clipboard and window-size events for the tab shown
    /// in the window they came from.
    fn on_input(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Key {
                window,
                pressed,
                key,
                modified,
                physical,
                location,
            } => {
                if let Some(tab) = self.tab_for_window(window) {
                    tab.key(pressed, &key, &modified, physical, location);
                }
            }
            Message::Button {
                window,
                pressed,
                button,
            } => {
                if let Some(tab) = self.tab_for_window(window) {
                    tab.button(pressed, button);
                }
            }
            Message::Scroll { window, delta } => {
                if let Some(tab) = self.tab_for_window(window) {
                    tab.scroll(delta);
                }
            }
            Message::WindowFocused(window) => {
                if self.tab_for_window(window).is_some_and(|t| t.is_desktop()) {
                    return iced::clipboard::read().map(move |text| {
                        cosmic::Action::App(Message::LocalClipboard(window, text))
                    });
                }
            }
            Message::LocalClipboard(window, Some(text)) => {
                if let Some(tab) = self.tab_for_window(window) {
                    tab.command(farsight_net::Command::Clipboard(text));
                }
            }
            Message::WindowResized(window, size) => self.on_resized(window, size),
            Message::ResizeTick => {
                for tab in &mut self.tabs {
                    if let Some((width, height)) = tab.pending_resize.take() {
                        tab.command(farsight_net::Command::Resize { width, height });
                    }
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn on_tab_message(&mut self, id: TabId, message: TabMessage) -> Task<Message> {
        let window = self.window_of(id);
        let Some(tab) = self.tab_mut(id) else {
            return Task::none();
        };
        let outcome = tab.update(message, window);
        self.apply(id, outcome)
    }

    fn apply(&mut self, id: TabId, outcome: Outcome) -> Task<Message> {
        match outcome {
            Outcome::None => self.update_title(),
            Outcome::Task(task) => Task::batch([task, self.update_title()]),
            Outcome::Close => self.close_tab(id),
            Outcome::Detach => self.detach(id),
        }
    }

    /// The window a tab is shown in (its own, or main when it's active there).
    fn window_of(&self, id: TabId) -> Option<window::Id> {
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        tab.window.or_else(|| {
            (self.active == Some(id))
                .then(|| self.core.main_window_id())
                .flatten()
        })
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    fn active_tab(&self) -> Option<&Tab> {
        let id = self.active?;
        self.tabs.iter().find(|t| t.id == id && t.window.is_none())
    }

    /// The tab that receives input from `window`.
    fn tab_for_window(&mut self, window: window::Id) -> Option<&mut Tab> {
        if self.core.main_window_id() == Some(window) {
            let id = self.active?;
            self.tabs
                .iter_mut()
                .find(|t| t.id == id && t.window.is_none())
        } else {
            self.tabs.iter_mut().find(|t| t.window == Some(window))
        }
    }

    fn open_tab(&mut self, address: ServerAddress, allow_insecure: bool) -> Task<Message> {
        self.counter += 1;
        let id = self.counter;
        let known = self.store.lookup(&address);
        let mut stored = known.password.map(|password| Remember {
            username: known.username,
            password,
        });
        if let (None, Some(path)) = (&stored, &self.options.password_file) {
            match farsight_vault::legacy::read_legacy_passwd(path) {
                Ok(password) => {
                    stored = Some(Remember {
                        username: None,
                        password,
                    });
                }
                Err(error) => tracing::warn!(%error, "could not read the password file"),
            }
        }
        let request = ConnectRequest {
            tab: id,
            host: address.host.clone(),
            port: address.port,
            view_only: self.options.view_only,
            shared: self.options.shared,
            quality: self.options.quality,
            compress: self.options.compress,
            allow_insecure,
            pinned: known.pinned,
        };
        let f8 = self.options.menu_key.eq_ignore_ascii_case("F8");
        self.tabs.push(Tab::new(request, address, stored, f8));
        let activate = self.activate(Some(id));
        if self.options.full_screen {
            let window = self.core.main_window_id();
            if let Some(tab) = self.tab_mut(id) {
                return Task::batch([activate, tab.set_full_screen(true, window)]);
            }
        }
        activate
    }

    fn activate(&mut self, id: Option<TabId>) -> Task<Message> {
        if let Some(previous) = self
            .active
            .and_then(|p| self.tabs.iter_mut().find(|t| t.id == p))
        {
            previous.release_keys();
        }
        self.active = id;
        let full_screen = self.active_tab().is_some_and(|t| t.full_screen);
        self.core.window.show_headerbar = !full_screen;
        self.update_title()
    }

    fn close_tab(&mut self, id: TabId) -> Task<Message> {
        let mut tasks = Vec::new();
        if let Some(tab) = self.tabs.iter().find(|t| t.id == id) {
            tab.command(farsight_net::Command::Disconnect);
            if let Some(window) = tab.window {
                tasks.push(window::close(window));
            } else if tab.full_screen
                && let Some(main) = self.core.main_window_id()
            {
                tasks.push(window::set_mode(main, window::Mode::Windowed));
            }
        }
        self.tabs.retain(|t| t.id != id);
        if self.active == Some(id) {
            let next = self
                .tabs
                .iter()
                .rev()
                .find(|t| t.window.is_none())
                .map(|t| t.id);
            tasks.push(self.activate(next));
        }
        Task::batch(tasks)
    }

    /// Moves a tab into its own window (U-7).
    fn detach(&mut self, id: TabId) -> Task<Message> {
        let (window, open) = window::open(window::Settings {
            size: Size::new(1280.0, 800.0),
            ..Default::default()
        });
        if let Some(tab) = self.tab_mut(id) {
            tab.window = Some(window);
        }
        let mut tasks = vec![open.map(|_| cosmic::Action::App(Message::Noop))];
        if self.active == Some(id) {
            let next = self
                .tabs
                .iter()
                .rev()
                .find(|t| t.window.is_none())
                .map(|t| t.id);
            tasks.push(self.activate(next));
        }
        Task::batch(tasks)
    }

    fn on_resized(&mut self, window: window::Id, size: Size) {
        if !self.options.remote_resize {
            return;
        }
        let is_main = self.core.main_window_id() == Some(window);
        let tab_bar = is_main && self.tabs.iter().any(|t| t.window.is_none());
        if let Some(tab) = self.tab_for_window(window) {
            // The header bar and tab bar take part of the main window unless full screen.
            let chrome = if tab.full_screen || !is_main {
                0.0
            } else {
                HEADER_HEIGHT + if tab_bar { TAB_BAR_HEIGHT } else { 0.0 }
            };
            tab.pending_resize = Some((to_u16(size.width), to_u16(size.height - chrome)));
        }
    }

    fn update_title(&mut self) -> Task<Message> {
        let title = match self.active_tab() {
            Some(tab) => format!("{} — Farsight", tab.title()),
            None => "Farsight".to_owned(),
        };
        self.set_header_title(title.clone());
        let mut tasks = Vec::new();
        if let Some(id) = self.core.main_window_id() {
            tasks.push(self.set_window_title(title, id));
        }
        let detached: Vec<(window::Id, String)> = self
            .tabs
            .iter()
            .filter_map(|t| t.window.map(|w| (w, format!("{} — Farsight", t.title()))))
            .collect();
        for (window, title) in detached {
            tasks.push(self.set_window_title(title, window));
        }
        Task::batch(tasks)
    }

    fn tab_bar(&self, tabs: &[&Tab]) -> Element<'_, Message> {
        let mut row = widget::Row::new()
            .spacing(4)
            .padding(4)
            .align_y(Alignment::Center);
        let home = if self.active.is_none() {
            widget::button::suggested("Home")
        } else {
            widget::button::standard("Home")
        };
        row = row.push(home.on_press(Message::ShowHome));
        for tab in tabs {
            let label = tab.title();
            let button = if self.active == Some(tab.id) {
                widget::button::suggested(label)
            } else {
                widget::button::standard(label)
            };
            row = row.push(
                widget::Row::new()
                    .push(button.on_press(Message::Activate(tab.id)))
                    .push(
                        widget::button::icon(widget::icon::from_name("window-close-symbolic"))
                            .tooltip("Disconnect")
                            .on_press(Message::CloseTab(tab.id)),
                    )
                    .align_y(Alignment::Center),
            );
        }
        row.into()
    }

    fn home_view(&self) -> Element<'_, Message> {
        match &self.home {
            Home::Unlock { creating, error } => self.unlock_view(*creating, error.as_deref()),
            Home::Connect => self.connect_view(),
            Home::Failed(reason) => centred(
                widget::Column::new()
                    .push(widget::text::title3("Couldn't connect"))
                    .push(widget::text::body(reason.clone()))
                    .push(widget::button::suggested("Back").on_press(Message::ShowHome))
                    .spacing(12)
                    .align_x(Alignment::Center),
            ),
        }
    }

    fn after_unlock(&mut self) -> Task<Message> {
        self.home = Home::Connect;
        self.scan_if_trusted();
        match self.options.server.clone() {
            Some(server) => self.open_tab(server, self.options.allow_insecure),
            None => Task::none(),
        }
    }

    fn on_nearby(&mut self, message: NearbyMessage) {
        match message {
            NearbyMessage::Discovery(discovery) => {
                let finished = matches!(discovery, Discovery::ProbeFinished(_));
                self.nearby.record(discovery);
                if finished {
                    self.scan_attempt = None;
                }
            }
            NearbyMessage::TrustNetworkAndScan => {
                if let Some(network) = self.network.clone() {
                    self.store.trust_network(&network);
                }
                self.start_scan();
            }
            NearbyMessage::Rescan => self.start_scan(),
            NearbyMessage::CancelScan => {
                self.scan_attempt = None;
                self.nearby.scan = None;
            }
            NearbyMessage::ForgetNetwork => {
                if let Some(network) = self.network.clone() {
                    self.store.forget_network(&network);
                }
                self.scan_attempt = None;
            }
            NearbyMessage::Add(address, label) => {
                self.store.add(&address, &label);
            }
        }
    }

    fn network_trusted(&self) -> bool {
        self.network
            .as_deref()
            .is_some_and(|n| self.store.is_trusted_network(n))
    }

    /// Scans automatically on networks the user marked as their own (ADR-0012).
    fn scan_if_trusted(&mut self) {
        if self.network_trusted() {
            self.start_scan();
        }
    }

    fn start_scan(&mut self) {
        self.counter += 1;
        self.scan_attempt = Some(self.counter);
        self.nearby.scan = Some(Scan::Running { done: 0, total: 0 });
    }

    // --- Tray (ADR-0013) ---

    fn start_tray(&mut self) -> Task<Message> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *TRAY_ACTIONS.lock().unwrap_or_else(PoisonError::into_inner) = Some(rx);
        let model = self.compute_tray_model();
        Task::future(async move {
            let (tray, availability) = ViewerTray::start(model, tx).await;
            cosmic::Action::App(Message::TrayStarted(
                TrayHandle(Arc::new(tray)),
                availability,
            ))
        })
    }

    fn compute_tray_model(&self) -> TrayModel {
        let sanitize = farsight_discovery::sanitize_name;
        let saved = if self.store.locked() {
            Vec::new()
        } else {
            self.store
                .search("")
                .into_iter()
                .filter(|e| e.saved)
                .take(10)
                .map(|e| TrayEntry {
                    id: e.address.display_name(),
                    label: sanitize(&e.label),
                })
                .collect()
        };
        let open = self
            .tabs
            .iter()
            .map(|t| TrayEntry {
                id: t.id.to_string(),
                label: sanitize(&t.title()),
            })
            .collect();
        let discovered = self
            .nearby
            .entries()
            .into_iter()
            .take(10)
            .map(|e| TrayEntry {
                id: e.address.display_name(),
                label: if e.label.is_empty() {
                    e.address.display_name()
                } else {
                    e.label
                },
            })
            .collect();
        TrayModel {
            saved,
            open,
            discovered,
            any_unencrypted: self
                .tabs
                .iter()
                .any(|t| t.connected.as_ref().is_some_and(|c| !c.encrypted)),
            hide_discovered: false,
            locked: self.store.locked(),
        }
    }

    fn sync_tray(&mut self) -> Task<Message> {
        let Some(tray) = self.tray.clone() else {
            return Task::none();
        };
        let model = self.compute_tray_model();
        if model == self.tray_model {
            return Task::none();
        }
        self.tray_model = model.clone();
        Task::future(async move {
            tray.0.update(model).await;
            cosmic::Action::App(Message::Noop)
        })
    }

    fn on_tray(&mut self, action: TrayAction) -> Task<Message> {
        match action {
            TrayAction::Show => self.focus_main(),
            TrayAction::Quit => iced::exit(),
            TrayAction::Raise(id) => {
                let Ok(id) = id.parse::<TabId>() else {
                    return Task::none();
                };
                match self.tabs.iter().find(|t| t.id == id).and_then(|t| t.window) {
                    Some(window) => window::gain_focus(window),
                    None => Task::batch([self.activate(Some(id)), self.focus_main()]),
                }
            }
            // Discovered computers go through the normal flow: identity is
            // checked before any credentials are sent (council condition).
            TrayAction::Connect(address) | TrayAction::Discovered(address) => {
                match ServerAddress::parse(&address) {
                    Ok(address) => Task::batch([
                        self.open_tab(address, self.options.allow_insecure),
                        self.focus_main(),
                    ]),
                    Err(_) => Task::none(),
                }
            }
        }
    }

    fn focus_main(&self) -> Task<Message> {
        self.core
            .main_window_id()
            .map_or_else(Task::none, window::gain_focus)
    }

    // --- Home views ---

    fn unlock_view(&self, creating: bool, error: Option<&str>) -> Element<'_, Message> {
        let (title, body, action) = if creating {
            (
                "Protect your saved computers",
                "Your desktop has no keyring, so Farsight needs a passphrase to encrypt the computers and \
                 passwords you save. You'll enter it each time Farsight starts.",
                "Create",
            )
        } else {
            (
                "Unlock saved computers",
                "Enter the passphrase that protects your saved computers.",
                "Unlock",
            )
        };
        let mut column = widget::Column::new()
            .push(widget::text::title3(title))
            .push(widget::text::body(body).width(Length::Fixed(480.0)))
            .push(
                widget::secure_input("Passphrase", &self.passphrase_input, None, true)
                    .on_input(Message::PassphraseChanged)
                    .on_submit(|_| Message::Unlock)
                    .width(Length::Fixed(320.0)),
            )
            .spacing(12)
            .align_x(Alignment::Center);
        if let Some(error) = error {
            column = column.push(widget::text::body(error.to_owned()));
        }
        centred(
            column.push(
                widget::Row::new()
                    .push(widget::button::standard("Skip for now").on_press(Message::SkipUnlock))
                    .push(widget::button::suggested(action).on_press(Message::Unlock))
                    .spacing(8),
            ),
        )
    }

    fn connect_view(&self) -> Element<'_, Message> {
        let mut column = widget::Column::new()
            .push(widget::text::title2("Connect to a computer"))
            .push(widget::text::body(
                "Enter its name or address, for example pop-os:1 or 192.168.1.20",
            ))
            .push(
                widget::search_input("Computer", &self.address_input)
                    .on_input(Message::AddressChanged)
                    .on_submit(|_| Message::Connect)
                    .width(Length::Fixed(360.0)),
            )
            .push(widget::button::suggested("Connect").on_press(Message::Connect))
            .spacing(12)
            .align_x(Alignment::Center);
        let entries = self.store.search(&self.address_input);
        if !entries.is_empty() {
            let mut list = widget::Column::new().spacing(4).width(Length::Fixed(360.0));
            for Entry {
                label,
                address,
                saved,
            } in entries
            {
                let text = if saved {
                    format!(
                        "{} — {}",
                        farsight_discovery::sanitize_name(&label),
                        address.display_name()
                    )
                } else {
                    format!("{} (recent)", address.display_name())
                };
                list = list.push(
                    widget::button::text(text)
                        .on_press(Message::Pick(address))
                        .width(Length::Fill),
                );
            }
            column = column
                .push(widget::text::heading("Your computers"))
                .push(list);
        }
        column = column.push(self.nearby_view());
        if let Some(problem) = self.store.problem() {
            column = column.push(widget::text::caption(problem.to_owned()));
        }
        centred(widget::scrollable(column))
    }

    fn nearby_view(&self) -> Element<'_, Message> {
        let mut column = widget::Column::new()
            .push(widget::text::heading("Computers on your network"))
            .spacing(6)
            .width(Length::Fixed(520.0));
        let entries = self.nearby.entries();
        if entries.is_empty() {
            column = column.push(widget::text::caption(
                "None found yet. Farsight lists computers that announce themselves; to find the rest, \
                 let it scan a network you own.",
            ));
        }
        for NearbyEntry {
            label,
            address,
            encryption,
        } in entries
        {
            let security = match encryption {
                Encryption::Offered => "encryption available",
                Encryption::NotOffered => "NOT encrypted",
                Encryption::Unknown => "not checked",
            };
            let name = if label.is_empty() {
                address.display_name()
            } else {
                label.clone()
            };
            column = column.push(
                widget::Row::new()
                    .push(
                        widget::text::body(format!(
                            "{name} — {} · identity not verified · {security}",
                            address.display_name()
                        ))
                        .width(Length::Fill),
                    )
                    .push(
                        widget::button::text("Add")
                            .on_press(Message::Nearby(NearbyMessage::Add(address.clone(), name))),
                    )
                    .push(widget::button::standard("Connect").on_press(Message::Pick(address)))
                    .spacing(8)
                    .align_y(Alignment::Center),
            );
        }
        let scan_controls: Element<'_, Message> = match (self.nearby.scan, self.network_trusted()) {
            (Some(Scan::Running { done, total }), _) => widget::Row::new()
                .push(widget::text::caption(format!("Scanning your network… {done} of {total}")))
                .push(widget::button::text("Cancel").on_press(Message::Nearby(NearbyMessage::CancelScan)))
                .spacing(8)
                .into(),
            (_, true) => widget::Row::new()
                .push(widget::button::text("Scan again").on_press(Message::Nearby(NearbyMessage::Rescan)))
                .push(widget::button::text("Stop scanning this network").on_press(Message::Nearby(NearbyMessage::ForgetNetwork)))
                .spacing(8)
                .into(),
            (_, false) if self.network.is_some() => widget::Column::new()
                .push(widget::text::caption(
                    "To find computers that don't announce themselves, Farsight can briefly check each device on \
                     this network for a VNC server. Only do this on a network you own: it never sends passwords, \
                     but scanning other people's networks may break their rules.",
                ))
                .push(
                    widget::button::standard("This is my network, scan it")
                        .on_press(Message::Nearby(NearbyMessage::TrustNetworkAndScan)),
                )
                .spacing(6)
                .into(),
            _ => widget::text::caption("Scanning isn't available on this network.").into(),
        };
        column = column.push(scan_controls);
        if let Some(problem) = &self.nearby.problem {
            column = column.push(widget::text::caption(problem.clone()));
        }
        column.into()
    }
}

/// Takes the tray's action receiver once and streams its picks.
fn tray_actions() -> impl iced::futures::Stream<Item = TrayAction> {
    iced::stream::channel(8, async move |mut output| {
        use iced::futures::SinkExt;
        let receiver = TRAY_ACTIONS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(mut receiver) = receiver else { return };
        while let Some(action) = receiver.recv().await {
            if output.send(action).await.is_err() {
                return;
            }
        }
    })
}

/// Turns window events into messages. Keys already handled by a widget
/// (e.g. typed into a text field) are not forwarded.
fn on_event(event: event::Event, status: event::Status, window: window::Id) -> Option<Message> {
    match event {
        event::Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            modified_key,
            physical_key,
            location,
            ..
        }) if status == event::Status::Ignored => Some(Message::Key {
            window,
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
            window,
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
                window,
                pressed: true,
                button,
            })
        }
        event::Event::Mouse(mouse::Event::ButtonReleased(button)) => Some(Message::Button {
            window,
            pressed: false,
            button,
        }),
        event::Event::Mouse(mouse::Event::WheelScrolled { delta })
            if status == event::Status::Ignored =>
        {
            Some(Message::Scroll { window, delta })
        }
        event::Event::Window(window::Event::Focused) => Some(Message::WindowFocused(window)),
        event::Event::Window(window::Event::Resized(size)) => {
            Some(Message::WindowResized(window, size))
        }
        _ => None,
    }
}
