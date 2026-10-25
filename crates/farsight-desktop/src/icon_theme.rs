//! Which icon theme the libcosmic apps should fall back to. libcosmic looks
//! for its own "Cosmic" theme, which only COSMIC desktops install; elsewhere
//! (e.g. Pop!_OS 22.04 GNOME) icons such as the window's close, maximise
//! and minimise buttons would be missing.

use std::path::{Path, PathBuf};

/// Themes to try, best first: COSMIC's own, then Pop!_OS's, then GNOME's.
const PREFERRED: [&str; 3] = ["Cosmic", "Pop", "Adwaita"];

/// The theme to pass to libcosmic as its default: `None` when its own
/// "Cosmic" theme is installed (then the desktop's configured theme applies,
/// whatever the desktop), otherwise the first installed of Pop and Adwaita.
#[must_use]
pub fn for_libcosmic() -> Option<&'static str> {
    fallback().filter(|&theme| theme != "Cosmic")
}

/// The first preferred theme installed on this computer, if any.
fn fallback() -> Option<&'static str> {
    let dirs = icon_dirs();
    PREFERRED
        .into_iter()
        .find(|theme| dirs.iter().any(|dir| installed(dir, theme)))
}

fn installed(icons: &Path, theme: &str) -> bool {
    icons.join(theme).join("index.theme").is_file()
}

/// `$XDG_DATA_HOME/icons`, then each `$XDG_DATA_DIRS/icons` (with the
/// usual defaults).
fn icon_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    home.into_iter()
        .chain(system.split(':').map(PathBuf::from))
        .map(|dir| dir.join("icons"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_theme_counts_only_with_its_index() {
        let dir = std::env::temp_dir().join(format!("farsight-icons-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Pop")).unwrap();
        assert!(!installed(&dir, "Pop"));
        std::fs::write(dir.join("Pop/index.theme"), "[Icon Theme]\n").unwrap();
        assert!(installed(&dir, "Pop"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
