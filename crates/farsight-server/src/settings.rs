//! The sharing settings file (`~/.config/farsight/agent/settings`): written
//! by the Farsight Sharing app, read by the live-desktop agent as its
//! defaults (its command line wins). One parser for both, so they agree.
//!
//! Format: `key = value` lines and `#` comments. Keys: `listen =
//! loopback|lan`, `announce = yes|no`, `view-only = yes|no`. Unknown keys
//! and values are ignored and keep the secure defaults, so a broken file
//! can't stop the agent or open it up.

/// Who may connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Loopback only (the default).
    ThisComputer,
    /// Other computers on the network.
    Network,
}

/// The settings the file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharingSettings {
    /// Who may connect.
    pub reach: Reach,
    /// Announce over mDNS on networks the user marked as theirs.
    pub announce: bool,
    /// Viewers can watch but not control.
    pub view_only: bool,
}

impl Default for SharingSettings {
    fn default() -> Self {
        Self {
            reach: Reach::ThisComputer,
            announce: true,
            view_only: false,
        }
    }
}

impl SharingSettings {
    /// Reads the file's text; anything not understood keeps the default.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"').to_ascii_lowercase();
            match (key.trim().to_ascii_lowercase().as_str(), value.as_str()) {
                ("listen", "lan") => settings.reach = Reach::Network,
                ("listen", "loopback") => settings.reach = Reach::ThisComputer,
                ("announce", v) => settings.announce = yes_no(v).unwrap_or(settings.announce),
                ("view-only", v) => settings.view_only = yes_no(v).unwrap_or(settings.view_only),
                _ => {}
            }
        }
        settings
    }

    /// The file's text.
    #[must_use]
    pub fn render(self) -> String {
        let word = |on: bool| if on { "yes" } else { "no" };
        let listen = match self.reach {
            Reach::ThisComputer => "loopback",
            Reach::Network => "lan",
        };
        format!(
            "# Written by Farsight Sharing; read by farsight-agent.\n\
             listen = {listen}\nannounce = {}\nview-only = {}\n",
            word(self.announce),
            word(self.view_only),
        )
    }
}

/// `Some(true)`/`Some(false)` for the usual yes/no words, `None` otherwise.
fn yes_no(value: &str) -> Option<bool> {
    match value {
        "yes" | "true" | "on" | "1" => Some(true),
        "no" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let settings = SharingSettings {
            reach: Reach::Network,
            announce: false,
            view_only: true,
        };
        assert_eq!(SharingSettings::parse(&settings.render()), settings);
        assert_eq!(SharingSettings::parse(""), SharingSettings::default());
    }

    #[test]
    fn unknown_words_keep_the_defaults() {
        let parsed =
            SharingSettings::parse("listen = everywhere\nannounce = maybe\nview-only = sure");
        assert_eq!(parsed, SharingSettings::default());
    }

    #[test]
    fn keys_and_values_ignore_case_and_comments() {
        let parsed = SharingSettings::parse("# listen = lan\nLISTEN = \"LAN\"\nView-Only = Yes");
        assert_eq!(parsed.reach, Reach::Network);
        assert!(parsed.view_only);
    }
}
