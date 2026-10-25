//! What the app reads and changes. Changes go through the same tools a user
//! would run (`systemctl --user`, `farsight-agent`, `farsight-passwd`), so
//! the agent's own rules (e.g. "network access needs a password") apply.

use std::io::Write;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use farsight_server::settings::SharingSettings;
use secrecy::{ExposeSecret, SecretString};

/// The systemd user unit that shares the desktop.
const UNIT: &str = "farsight-agent.service";
/// Default VNC port (display :0).
pub const PORT: u16 = 5900;

/// Everything the window shows.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub sharing: bool,
    pub settings: SharingSettings,
    /// `None` when the network can't be identified (no gateway).
    pub network_trusted: Option<bool>,
    pub host: String,
    pub addresses: Vec<IpAddr>,
    pub has_password: bool,
    pub fingerprints: Option<String>,
}

fn agent_dir() -> Option<PathBuf> {
    Some(
        farsight_vault::paths::config_home()?
            .join("farsight")
            .join("agent"),
    )
}

pub fn snapshot() -> Snapshot {
    let dir = agent_dir();
    let settings = dir
        .as_ref()
        .and_then(|d| std::fs::read_to_string(d.join("settings")).ok())
        .map_or_else(SharingSettings::default, |t| SharingSettings::parse(&t));
    Snapshot {
        sharing: systemctl(&["is-active", "--quiet", UNIT]),
        settings,
        network_trusted: network_trusted(dir.as_ref()),
        host: std::fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|h| h.trim().to_owned())
            .unwrap_or_default(),
        addresses: addresses(),
        has_password: output("farsight-passwd", &["user", "list"])
            .is_some_and(|text| has_login(&text)),
        fingerprints: output("farsight-agent", &["--show-identity"]),
    }
}

/// Whether `farsight-passwd user list` shows a user (lines before its
/// VncAuth status line, other than the "No users yet" hint).
fn has_login(listing: &str) -> bool {
    listing
        .lines()
        .take_while(|l| !l.starts_with("VNC"))
        .any(|l| !l.trim().is_empty() && !l.starts_with("No users"))
}

/// Whether the current network is in the agent's trusted list.
fn network_trusted(dir: Option<&PathBuf>) -> Option<bool> {
    let here = farsight_discovery::current_networks();
    if here.is_empty() {
        return None;
    }
    let trusted = dir
        .and_then(|d| std::fs::read_to_string(d.join("trusted-networks")).ok())
        .unwrap_or_default();
    Some(
        here.iter()
            .any(|n| trusted.lines().any(|l| l.trim() == n.as_str())),
    )
}

/// This computer's network addresses (not loopback, link-local or
/// container bridges).
pub fn addresses() -> Vec<IpAddr> {
    let mut found: Vec<IpAddr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !is_virtual_interface(&i.name))
        .map(|i| i.ip())
        .filter(|ip| match ip {
            IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local(),
            IpAddr::V6(v6) => !v6.is_loopback() && (v6.segments()[0] & 0xffc0) != 0xfe80,
        })
        .collect();
    found.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    found.dedup();
    worth_showing(found)
}

/// IPv4 addresses when there are any: they're what people type, and IPv6
/// brings a changing list of temporary addresses.
fn worth_showing(sorted: Vec<IpAddr>) -> Vec<IpAddr> {
    if sorted.iter().any(IpAddr::is_ipv4) {
        sorted.into_iter().filter(IpAddr::is_ipv4).collect()
    } else {
        sorted
    }
}

/// Container and VM bridges (Docker, libvirt, LXD, Podman): their addresses
/// can't be reached from other computers, so listing them only confuses.
fn is_virtual_interface(name: &str) -> bool {
    [
        "docker", "br-", "veth", "virbr", "lxdbr", "cni", "podman", "flannel",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

fn systemctl(args: &[&str]) -> bool {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs a tool and turns failure into its message.
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    let text = String::from_utf8_lossy(if out.status.success() {
        &out.stdout
    } else {
        &out.stderr
    })
    .trim()
    .to_owned();
    if out.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

/// Sharing turned on (now and at every login) or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sharing {
    On,
    Off,
}

/// Starts sharing now and at every login, or stops it.
pub fn set_sharing(sharing: Sharing) -> Result<String, String> {
    let on = sharing == Sharing::On;
    let verb = if on { "enable" } else { "disable" };
    run("systemctl", &["--user", verb, "--now", UNIT])?;
    if on && !systemctl(&["is-active", "--quiet", UNIT]) {
        let log = run(
            "journalctl",
            &["--user", "-u", UNIT, "-n", "5", "--no-pager", "-o", "cat"],
        )
        .unwrap_or_default();
        return Err(format!("Sharing didn't start. {log}"));
    }
    Ok(if on {
        "Sharing is on."
    } else {
        "Sharing is off."
    }
    .into())
}

/// Saves the settings and restarts sharing if it is on, so they apply.
pub fn save_settings(settings: SharingSettings) -> Result<String, String> {
    let dir = agent_dir().ok_or("no config folder")?;
    write_private(&dir, "settings", &settings.render()).map_err(|e| e.to_string())?;
    if systemctl(&["is-active", "--quiet", UNIT]) {
        run("systemctl", &["--user", "restart", UNIT])?;
    }
    Ok("Saved.".into())
}

/// Writes `dir/name` owner-only (the folder too, like the agent makes it),
/// replacing it in one step so the agent never reads half a file.
fn write_private(dir: &std::path::Path, name: &str, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let temporary = dir.join(format!(".{name}.new"));
    let _ = std::fs::remove_file(&temporary);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temporary, dir.join(name))
}

/// Marks the current network as yours (the agent announces itself there).
pub fn trust_network() -> Result<String, String> {
    run("farsight-agent", &["--trust-this-network"])
}

/// Forgets that the current network is yours.
pub fn untrust_network() -> Result<String, String> {
    run("farsight-agent", &["--untrust-this-network"])
}

/// Sets this user's password for connecting from other computers.
pub fn set_password(password: &SecretString) -> Result<String, String> {
    let user = std::env::var("USER").map_err(|_| "who are you? (no $USER)")?;
    let mut child = Command::new("farsight-passwd")
        .args(["--stdin", "user", "add", &user])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("farsight-passwd: {e}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        writeln!(stdin, "{}", password.expose_secret()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        // Apply it without dropping anyone (M9 reload).
        let running = systemctl(&["is-active", "--quiet", UNIT]);
        if running && !systemctl(&["reload", UNIT]) {
            return Err(format!(
                "Password set for {user}, but sharing didn't pick it up: turn sharing off and on."
            ));
        }
        Ok(format!("Password set for {user}."))
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv6_is_shown_only_without_ipv4() {
        let v4: IpAddr = "192.168.1.165".parse().unwrap();
        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(worth_showing(vec![v4, v6]), [v4]);
        assert_eq!(worth_showing(vec![v6]), [v6]);
    }

    #[test]
    fn settings_are_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("agent");
        write_private(&dir, "settings", "listen = lan\n").unwrap();
        write_private(&dir, "settings", "listen = loopback\n").unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("settings")), 0o600);
        assert_eq!(
            std::fs::read_to_string(dir.join("settings")).unwrap(),
            "listen = loopback\n"
        );
    }

    #[test]
    fn container_bridges_are_not_listed_as_addresses() {
        assert!(is_virtual_interface("docker0"));
        assert!(is_virtual_interface("br-3f2a"));
        assert!(!is_virtual_interface("wlp3s0"));
        assert!(!is_virtual_interface("enp5s0"));
    }
}
