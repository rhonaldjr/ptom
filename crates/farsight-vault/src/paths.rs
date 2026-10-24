//! Default file locations, following the XDG Base Directory specification
//! on both Linux and macOS.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

/// `$XDG_DATA_HOME`, else `$HOME/.local/share`.
#[must_use]
pub fn data_home() -> Option<PathBuf> {
    xdg_dir(env::var_os("XDG_DATA_HOME"), ".local/share")
}

/// `$XDG_CONFIG_HOME`, else `$HOME/.config`.
#[must_use]
pub fn config_home() -> Option<PathBuf> {
    xdg_dir(env::var_os("XDG_CONFIG_HOME"), ".config")
}

/// `$XDG_DATA_HOME/farsight/vault.bin`.
#[must_use]
pub fn vault_file() -> Option<PathBuf> {
    data_home().map(|d| d.join("farsight").join("vault.bin"))
}

/// `$XDG_CONFIG_HOME/farsight/agent-credentials.toml`.
#[must_use]
pub fn agent_credentials_file() -> Option<PathBuf> {
    config_home().map(|d| d.join("farsight").join("agent-credentials.toml"))
}

/// `$HOME/.vnc/passwd`, the legacy `vncpasswd` location.
#[must_use]
pub fn legacy_passwd_file() -> Option<PathBuf> {
    home().map(|h| h.join(".vnc").join("passwd"))
}

fn home() -> Option<PathBuf> {
    absolute(env::var_os("HOME"))
}

/// The XDG spec says relative values must be ignored.
fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|p| p.is_absolute())
}

fn xdg_dir(value: Option<OsString>, fallback: &str) -> Option<PathBuf> {
    absolute(value).or_else(|| home().map(|h| h.join(fallback)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_relative_and_empty_values() {
        assert_eq!(absolute(Some("relative/dir".into())), None);
        assert_eq!(absolute(Some(OsString::new())), None);
        assert_eq!(
            absolute(Some("/abs/dir".into())),
            Some(PathBuf::from("/abs/dir"))
        );
    }
}
