//! The viewer's tray icon (VWR-15, ADR-0013): a menu over the same view
//! model the main window shows. Every action here is also reachable from the
//! main window; the tray is only a shortcut.

use tokio::sync::mpsc::UnboundedSender;

/// One menu entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayEntry {
    /// Opaque id the app uses to find the computer or session.
    pub id: String,
    /// Label shown (already sanitised by the caller).
    pub label: String,
}

/// What the tray shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrayModel {
    /// Saved computers (quick connect). Empty while the vault is locked.
    pub saved: Vec<TrayEntry>,
    /// Open sessions (raise).
    pub open: Vec<TrayEntry>,
    /// Computers found on the network (unverified).
    pub discovered: Vec<TrayEntry>,
    /// At least one open session is unencrypted (shown in the tooltip).
    pub any_unencrypted: bool,
    /// Hide discovered computers from the tray (privacy setting).
    pub hide_discovered: bool,
    /// Saved computers wait for a passphrase: show only "Unlock…" for them.
    pub locked: bool,
}

/// What the user picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    /// Connect to a saved computer.
    Connect(String),
    /// Bring an open session to the front.
    Raise(String),
    /// Open the "add / verify" flow for a discovered computer. Never
    /// connects with saved credentials directly (council condition).
    Discovered(String),
    /// Show the main window.
    Show,
    /// Quit the viewer.
    Quit,
}

/// Whether a tray host exists (ADR-0013: an explicit, tested state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAvailability {
    /// The icon is shown.
    Available,
    /// No `StatusNotifierItem` host (e.g. vanilla GNOME): everything stays
    /// reachable from the main window.
    NoHost,
}

/// Builds the menu as plain data, so it can be tested without D-Bus.
#[must_use]
pub fn menu_lines(model: &TrayModel) -> Vec<(String, Option<TrayAction>)> {
    let mut lines = vec![("Show Farsight".to_owned(), Some(TrayAction::Show))];
    if !model.open.is_empty() {
        lines.push(("Open".to_owned(), None));
        lines.extend(model.open.iter().map(|e| {
            (
                format!("  {}", e.label),
                Some(TrayAction::Raise(e.id.clone())),
            )
        }));
    }
    if model.locked {
        lines.push(("Unlock saved computers…".to_owned(), Some(TrayAction::Show)));
    } else if !model.saved.is_empty() {
        lines.push(("Your computers".to_owned(), None));
        lines.extend(model.saved.iter().map(|e| {
            (
                format!("  {}", e.label),
                Some(TrayAction::Connect(e.id.clone())),
            )
        }));
    }
    if !model.hide_discovered && !model.discovered.is_empty() {
        lines.push(("On your network (not verified)".to_owned(), None));
        lines.extend(model.discovered.iter().map(|e| {
            (
                format!("  {}", e.label),
                Some(TrayAction::Discovered(e.id.clone())),
            )
        }));
    }
    lines.push(("Quit".to_owned(), Some(TrayAction::Quit)));
    lines
}

#[cfg(target_os = "linux")]
struct ViewerTrayIcon {
    model: TrayModel,
    actions: UnboundedSender<TrayAction>,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for ViewerTrayIcon {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "farsight-viewer".into()
    }

    fn title(&self) -> String {
        "Farsight".into()
    }

    fn icon_name(&self) -> String {
        "video-display".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let description = match (self.model.open.len(), self.model.any_unencrypted) {
            (0, _) => "No open sessions".to_owned(),
            (n, false) => format!("{n} open, encrypted"),
            (n, true) => format!("{n} open, some NOT encrypted"),
        };
        ksni::ToolTip {
            title: "Farsight".into(),
            description,
            ..ksni::ToolTip::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        menu_lines(&self.model)
            .into_iter()
            .map(|(label, action)| match action {
                None => ksni::menu::StandardItem {
                    label,
                    enabled: false,
                    ..Default::default()
                }
                .into(),
                Some(action) => ksni::menu::StandardItem {
                    label,
                    activate: Box::new(move |tray: &mut Self| {
                        let _ = tray.actions.send(action.clone());
                    }),
                    ..Default::default()
                }
                .into(),
            })
            .collect()
    }
}

/// A running viewer tray icon.
pub struct ViewerTray {
    #[cfg(target_os = "linux")]
    handle: Option<ksni::Handle<ViewerTrayIcon>>,
}

impl ViewerTray {
    /// Starts the icon; picks go to `actions`.
    pub async fn start(
        model: TrayModel,
        actions: UnboundedSender<TrayAction>,
    ) -> (Self, TrayAvailability) {
        #[cfg(target_os = "linux")]
        {
            use ksni::TrayMethods;
            match (ViewerTrayIcon { model, actions }).spawn().await {
                Ok(handle) => (
                    Self {
                        handle: Some(handle),
                    },
                    TrayAvailability::Available,
                ),
                Err(error) => {
                    tracing::info!(%error, "no tray host; the main window has every action");
                    (Self { handle: None }, TrayAvailability::NoHost)
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (model, actions);
            (Self {}, TrayAvailability::NoHost)
        }
    }

    /// Replaces what the menu shows.
    pub async fn update(&self, model: TrayModel) {
        #[cfg(target_os = "linux")]
        if let Some(handle) = &self.handle {
            handle.update(|tray| tray.model = model).await;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = model;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> TrayEntry {
        TrayEntry {
            id: id.into(),
            label: id.to_uppercase(),
        }
    }

    #[test]
    fn empty_model_has_show_and_quit() {
        let lines = menu_lines(&TrayModel::default());
        assert_eq!(
            lines.first().map(|l| l.1.clone()),
            Some(Some(TrayAction::Show))
        );
        assert_eq!(
            lines.last().map(|l| l.1.clone()),
            Some(Some(TrayAction::Quit))
        );
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn sections_and_actions() {
        let model = TrayModel {
            saved: vec![entry("desk")],
            open: vec![entry("lab")],
            discovered: vec![entry("nas")],
            ..TrayModel::default()
        };
        let actions: Vec<_> = menu_lines(&model).into_iter().filter_map(|l| l.1).collect();
        assert!(actions.contains(&TrayAction::Connect("desk".into())));
        assert!(actions.contains(&TrayAction::Raise("lab".into())));
        assert!(actions.contains(&TrayAction::Discovered("nas".into())));
    }

    #[test]
    fn locked_vault_shows_only_unlock() {
        let model = TrayModel {
            saved: vec![entry("desk")],
            locked: true,
            ..TrayModel::default()
        };
        let lines = menu_lines(&model);
        assert!(lines.iter().any(|l| l.0.starts_with("Unlock")));
        assert!(!lines.iter().any(|l| l.0.contains("DESK")));
    }

    #[test]
    fn discovered_can_be_hidden() {
        let model = TrayModel {
            discovered: vec![entry("nas")],
            hide_discovered: true,
            ..TrayModel::default()
        };
        assert!(!menu_lines(&model).iter().any(|l| l.0.contains("NAS")));
    }
}
