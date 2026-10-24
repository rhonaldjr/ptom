//! Running the user's startup script inside the new desktop (U-10).

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// What runs when there is no `headless-startup` script yet. The agent
/// writes it to the user's config so it can be edited, like TigerVNC's
/// `xstartup`. It opens a terminal and a file manager, whichever exist.
pub const DEFAULT_STARTUP_SCRIPT: &str = r#"#!/bin/sh
# Farsight headless desktop startup. Runs when a headless desktop starts;
# edit it to start the apps you want. WAYLAND_DISPLAY and DISPLAY are set.
for app in cosmic-term gnome-terminal konsole xfce4-terminal foot xterm; do
  if command -v "$app" >/dev/null 2>&1; then "$app" & break; fi
done
for app in cosmic-files nautilus dolphin thunar pcmanfm; do
  if command -v "$app" >/dev/null 2>&1; then "$app" & break; fi
done
wait
"#;

/// Starts `script` with the desktop's displays in its environment. Apps get
/// their own D-Bus session (when `dbus-run-session` exists) so single-window
/// apps don't open on the user's main desktop instead.
pub(crate) fn start(
    script: &Path,
    wayland_display: &str,
    x_display: Option<&str>,
    umask: Option<u32>,
) -> std::io::Result<Child> {
    // `sh` restores the user's umask (the agent itself runs with 077).
    let mut command = Command::new("/bin/sh");
    let umask = umask.map_or_else(|| "022".to_owned(), |m| format!("{m:03o}"));
    command
        .arg("-c")
        .arg(format!("umask {umask}; exec \"$@\""))
        .arg("sh");
    if on_path("dbus-run-session") {
        command.arg("dbus-run-session").arg("--");
    }
    command.arg(script);
    command
        .env("WAYLAND_DISPLAY", wayland_display)
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        // Its own process group, so stopping the desktop stops all of it.
        .process_group(0);
    if let Some(x_display) = x_display {
        command.env("DISPLAY", x_display);
    }
    command.spawn()
}

/// Oldest Xwayland that tells the compositor which surface belongs to which
/// X11 window in a way Smithay supports (the `xwayland_shell_v1` protocol).
const MIN_XWAYLAND: (u32, u32) = (23, 1);

/// Whether the installed Xwayland can show X11 apps in a headless desktop.
#[must_use]
pub fn xwayland_supported() -> bool {
    let Ok(output) = Command::new("Xwayland")
        .arg("-version")
        .stderr(Stdio::piped())
        .output()
    else {
        return false;
    };
    let text = String::from_utf8_lossy(&output.stderr) + String::from_utf8_lossy(&output.stdout);
    xwayland_version(&text).is_some_and(|version| version >= MIN_XWAYLAND)
}

/// Parses "… Xwayland Version 23.2.6 …".
fn xwayland_version(text: &str) -> Option<(u32, u32)> {
    let rest = text.split("Version ").nth(1)?;
    let mut parts = rest
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty());
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xwayland_versions_are_read() {
        assert_eq!(
            xwayland_version("The X.Org Foundation Xwayland Version 23.2.6 (12302006)"),
            Some((23, 2))
        );
        assert_eq!(xwayland_version("Xwayland Version 22.1.1"), Some((22, 1)));
        assert_eq!(xwayland_version("nothing here"), None);
        assert!((22, 1) < MIN_XWAYLAND && (23, 1) >= MIN_XWAYLAND && (24, 0) >= MIN_XWAYLAND);
    }

    #[test]
    fn the_script_sees_the_desktop_displays() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("env");
        let script = dir.path().join("startup");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$WAYLAND_DISPLAY $DISPLAY $XDG_SESSION_TYPE $(umask)\" > {}\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        start(&script, "wayland-9", Some(":9"), Some(0o027))
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(out).unwrap().trim(),
            "wayland-9 :9 wayland 0027"
        );
    }
}
