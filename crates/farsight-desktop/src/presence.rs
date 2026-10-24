//! The agent's "you are being viewed" tray icon (SRV-12, U-4).

use std::sync::Arc;

/// Called when the person at the screen chooses "Disconnect everyone".
pub type DisconnectAll = Arc<dyn Fn() + Send + Sync>;

#[cfg(target_os = "linux")]
struct PresenceTray {
    viewers: Vec<String>,
    disconnect_all: DisconnectAll,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for PresenceTray {
    fn id(&self) -> String {
        "farsight-agent".into()
    }

    fn title(&self) -> String {
        "Farsight screen sharing".into()
    }

    fn icon_name(&self) -> String {
        if self.viewers.is_empty() {
            "video-display".into()
        } else {
            "media-record".into()
        }
    }

    fn status(&self) -> ksni::Status {
        if self.viewers.is_empty() {
            ksni::Status::Active
        } else {
            ksni::Status::NeedsAttention
        }
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: summary(&self.viewers),
            ..ksni::ToolTip::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let mut items: Vec<ksni::MenuItem<Self>> = vec![
            ksni::menu::StandardItem {
                label: summary(&self.viewers),
                enabled: false,
                ..ksni::menu::StandardItem::default()
            }
            .into(),
        ];
        for viewer in &self.viewers {
            items.push(
                ksni::menu::StandardItem {
                    label: format!("  {viewer}"),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(ksni::MenuItem::Separator);
        items.push(
            ksni::menu::StandardItem {
                label: "Disconnect everyone".into(),
                enabled: !self.viewers.is_empty(),
                icon_name: "process-stop".into(),
                activate: Box::new(|tray: &mut Self| (tray.disconnect_all)()),
                ..ksni::menu::StandardItem::default()
            }
            .into(),
        );
        items
    }
}

/// One line describing who is connected.
#[must_use]
pub fn summary(viewers: &[String]) -> String {
    match viewers.len() {
        0 => "No one is viewing this screen".into(),
        1 => "1 person is viewing this screen".into(),
        n => format!("{n} people are viewing this screen"),
    }
}

/// A running presence indicator. Without a tray host (e.g. vanilla GNOME)
/// it is inert; notifications still announce every connection.
pub struct Presence {
    #[cfg(target_os = "linux")]
    handle: Option<ksni::Handle<PresenceTray>>,
}

impl Presence {
    /// Starts the tray icon.
    pub async fn start(disconnect_all: DisconnectAll) -> Self {
        #[cfg(target_os = "linux")]
        {
            use ksni::TrayMethods;
            let tray = PresenceTray {
                viewers: Vec::new(),
                disconnect_all,
            };
            match tray.spawn().await {
                Ok(handle) => Self {
                    handle: Some(handle),
                },
                Err(error) => {
                    tracing::info!(%error, "no tray host; relying on notifications");
                    Self { handle: None }
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = disconnect_all;
            Self {}
        }
    }

    /// Updates the list of connected viewers.
    pub async fn set_viewers(&self, viewers: Vec<String>) {
        #[cfg(target_os = "linux")]
        if let Some(handle) = &self.handle {
            handle.update(|tray| tray.viewers = viewers).await;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = viewers;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_wording() {
        assert_eq!(summary(&[]), "No one is viewing this screen");
        assert_eq!(summary(&["a".into()]), "1 person is viewing this screen");
        assert_eq!(
            summary(&["a".into(), "b".into()]),
            "2 people are viewing this screen"
        );
    }
}
