//! Farsight Sharing: a small window for the agent (user feedback after the
//! first install): turn sharing on or off, choose who may connect, mark this
//! network as yours, set your password, and see this computer's name,
//! addresses and fingerprints. The agent itself still runs as a systemd
//! user service; this only drives it.

mod system;

use std::process::ExitCode;

use cosmic::app::{Core, Settings as AppSettings, Task};
use cosmic::iced::{Alignment, Length, Size};
use cosmic::{Element, executor, widget};
use farsight_server::settings::Reach;
use secrecy::SecretString;
use zeroize::Zeroizing;

use crate::system::{PORT, Sharing, Snapshot};

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let settings = AppSettings::default()
        .size(Size::new(640.0, 760.0))
        .default_icon_theme(farsight_desktop::icon_theme::for_libcosmic().unwrap_or_default());
    match cosmic::app::run::<App>(settings, ()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("farsight-sharing: {error}");
            ExitCode::FAILURE
        }
    }
}

/// An action's result: `Ok` message or `Err` problem.
type Feedback = Result<String, String>;

#[derive(Clone)]
enum Message {
    Refresh,
    Sharing(bool),
    Reach(Reach),
    Announce(bool),
    ViewOnly(bool),
    Trust(bool),
    PasswordInput(String),
    SavePassword,
    /// Background work finished: its result and the fresh state.
    Done(Option<Feedback>, Box<Snapshot>),
}

/// Never prints the password being typed.
impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PasswordInput(_) => f.write_str("PasswordInput(..)"),
            Self::Done(..) => f.write_str("Done(..)"),
            Self::Refresh => f.write_str("Refresh"),
            Self::Sharing(on) => write!(f, "Sharing({on})"),
            Self::Reach(reach) => write!(f, "Reach({reach:?})"),
            Self::Announce(on) => write!(f, "Announce({on})"),
            Self::ViewOnly(on) => write!(f, "ViewOnly({on})"),
            Self::Trust(on) => write!(f, "Trust({on})"),
            Self::SavePassword => f.write_str("SavePassword"),
        }
    }
}

struct App {
    core: Core,
    state: Snapshot,
    password: Zeroizing<String>,
    /// The last action's result.
    feedback: Option<Feedback>,
    /// Background work is running (systemctl and friends can take seconds).
    busy: bool,
}

impl cosmic::Application for App {
    type Executor = executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "ca.akrotechnologies.FarsightSharing";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, (): ()) -> (Self, Task<Message>) {
        let mut app = Self {
            core,
            state: Snapshot::default(),
            password: Zeroizing::new(String::new()),
            feedback: None,
            busy: false,
        };
        let task = app.in_background(|| None);
        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let mut settings = self.state.settings;
        match message {
            Message::Done(feedback, state) => {
                self.busy = false;
                self.state = *state;
                if feedback.is_some() {
                    self.feedback = feedback;
                }
                Task::none()
            }
            Message::PasswordInput(text) => {
                *self.password = text;
                Task::none()
            }
            _ if self.busy => Task::none(),
            Message::Refresh => self.in_background(|| None),
            Message::Sharing(on) => {
                let sharing = if on { Sharing::On } else { Sharing::Off };
                self.in_background(move || Some(system::set_sharing(sharing)))
            }
            Message::Reach(Reach::Network) if !self.state.has_password => self.refuse(
                "Set a password below first: connections from the network always need one.",
            ),
            Message::Reach(reach) => {
                settings.reach = reach;
                self.in_background(move || Some(system::save_settings(settings)))
            }
            Message::Announce(on) => {
                settings.announce = on;
                self.in_background(move || Some(system::save_settings(settings)))
            }
            Message::ViewOnly(on) => {
                settings.view_only = on;
                self.in_background(move || Some(system::save_settings(settings)))
            }
            Message::Trust(true) => self.in_background(|| Some(system::trust_network())),
            Message::Trust(false) => self.in_background(|| Some(system::untrust_network())),
            Message::SavePassword if self.password.chars().count() < 8 => {
                self.refuse("Use at least 8 characters.")
            }
            Message::SavePassword => {
                let password = SecretString::from(std::mem::take(&mut *self.password));
                self.in_background(move || Some(system::set_password(&password)))
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let s = &self.state;
        let mut page = widget::Column::new()
            .spacing(16)
            .padding(24)
            .width(Length::Fill)
            .push(widget::text::title3("Share this computer"))
            .push(widget::settings::item(
                match (s.sharing, &s.problem) {
                    (true, None) => "Sharing is on: others can connect with a password",
                    (true, Some(_)) => "Sharing is on, but it isn't working",
                    (false, _) => "Sharing is off",
                },
                widget::toggler(s.sharing).on_toggle(Message::Sharing),
            ));
        if let Some(problem) = &s.problem {
            page = page.push(
                widget::Column::new()
                    .spacing(4)
                    .push(widget::text::heading("⚠ The sharing agent keeps stopping"))
                    .push(widget::text::body(problem.clone()))
                    .push(widget::text::caption(
                        "Full log: journalctl --user -u farsight-agent",
                    )),
            );
        }
        page = page.push(self.reach_section()).push(self.network_section());
        page = page.push(self.password_section());
        page = page.push(self.address_section());
        if self.busy {
            page = page.push(widget::text::body("Working…"));
        } else if let Some(feedback) = &self.feedback {
            page = page.push(match feedback {
                Ok(text) => widget::text::body(text.clone()),
                Err(problem) => widget::text::body(format!("⚠ {problem}")),
            });
        }
        page = page.push(widget::button::text("Refresh").on_press(Message::Refresh));
        widget::scrollable(page).into()
    }
}

impl App {
    /// Runs `job` (which may wait on `systemctl` and friends) off the UI
    /// thread, then reads the state again.
    fn in_background(
        &mut self,
        job: impl FnOnce() -> Option<Feedback> + Send + 'static,
    ) -> Task<Message> {
        self.busy = true;
        Task::future(async move {
            let (feedback, state) =
                tokio::task::spawn_blocking(move || (job(), system::snapshot()))
                    .await
                    .unwrap_or_else(|_| {
                        (
                            Some(Err("Something went wrong.".into())),
                            Snapshot::default(),
                        )
                    });
            cosmic::Action::App(Message::Done(feedback, Box::new(state)))
        })
    }

    fn refuse(&mut self, problem: &str) -> Task<Message> {
        self.feedback = Some(Err(problem.into()));
        Task::none()
    }

    fn reach_section(&self) -> Element<'_, Message> {
        let settings = self.state.settings;
        widget::Column::new()
            .spacing(8)
            .push(widget::text::heading("Who can connect"))
            .push(widget::radio(
                "Only this computer (safest; for testing or SSH tunnels)",
                Reach::ThisComputer,
                Some(settings.reach),
                Message::Reach,
            ))
            .push(widget::radio(
                "Computers on my network (always encrypted, password required)",
                Reach::Network,
                Some(settings.reach),
                Message::Reach,
            ))
            .push(widget::settings::item(
                "Viewers can watch but not control",
                widget::toggler(settings.view_only).on_toggle(Message::ViewOnly),
            ))
            .into()
    }

    fn network_section(&self) -> Element<'_, Message> {
        let s = &self.state;
        let mut column = widget::Column::new()
            .spacing(8)
            .push(widget::text::heading("Being found on this network"));
        column = match s.network_trusted {
            None => column.push(widget::text::caption(
                "Farsight can't tell which network this is (no router found).",
            )),
            Some(true) => column
                .push(widget::text::body(
                    "This network is marked as yours: Farsight viewers here can find this computer.",
                ))
                .push(widget::button::text("This isn't my network").on_press(Message::Trust(false))),
            Some(false) => column
                .push(widget::text::body(
                    "Viewers on this network won't see this computer in their list. Mark it as yours \
                     only at home or at work, never on public Wi-Fi.",
                ))
                .push(widget::button::standard("This is my network").on_press(Message::Trust(true))),
        };
        if s.settings.reach == Reach::ThisComputer {
            column = column.push(widget::text::caption(
                "Viewers can only find (and reach) this computer once \"Computers on my network\" is chosen.",
            ));
        }
        column
            .push(widget::settings::item(
                "Announce this computer on networks marked as mine",
                widget::toggler(s.settings.announce).on_toggle(Message::Announce),
            ))
            .into()
    }

    fn password_section(&self) -> Element<'_, Message> {
        let s = &self.state;
        let status = if s.has_password {
            "Your password is set. Change it here:"
        } else {
            "Set a password for connecting from other computers:"
        };
        widget::Column::new()
            .spacing(8)
            .push(widget::text::heading("Password"))
            .push(widget::text::body(status))
            .push(
                widget::Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(
                        widget::secure_input("New password", self.password.as_str(), None, true)
                            .on_input(Message::PasswordInput)
                            .on_submit(|_| Message::SavePassword),
                    )
                    .push(widget::button::standard("Set password").on_press(Message::SavePassword)),
            )
            .into()
    }

    fn address_section(&self) -> Element<'_, Message> {
        let s = &self.state;
        let mut column = widget::Column::new()
            .spacing(6)
            .push(widget::text::heading("How to reach this computer"))
            .push(widget::text::body(format!("Name: {}", s.host)));
        if s.addresses.is_empty() {
            column = column.push(widget::text::body("No network addresses (not connected?)"));
        }
        for address in &s.addresses {
            let shown = match address {
                std::net::IpAddr::V6(v6) => format!("[{v6}]"),
                std::net::IpAddr::V4(v4) => v4.to_string(),
            };
            column = column.push(widget::text::body(format!(
                "In a viewer: {shown}  (port {PORT}, display :0)"
            )));
        }
        if let Some(fingerprints) = &s.fingerprints {
            column = column
                .push(widget::text::caption(
                    "When a viewer first connects it shows a fingerprint; it should match one of these:",
                ))
                .push(widget::text::monotext(fingerprints.trim().to_owned()));
        }
        column.into()
    }
}
