//! `vncserver` compatibility (U-11): starts, stops and lists headless
//! desktops running in the background, like TigerVNC's `vncserver`.
//! For desktops that start at login, the systemd units are the better way.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::compat::VncserverCommand;
use crate::options::Listen;

/// How long a new desktop gets to fail (e.g. no password set) before we
/// report success.
const STARTUP_CHECK: Duration = Duration::from_millis(1500);

pub const USAGE: &str = "\
Usage: vncserver [:N] [-geometry WxH] [-localhost yes|no]
       vncserver -kill :N
       vncserver -list

Starts a Farsight headless desktop in the background (display :N listens on
port 5900+N). Run `farsight-passwd user add $USER` first. To start one at
login instead, use `systemctl --user enable --now farsight-headless@590N`.
";

pub fn run(command: VncserverCommand) -> Result<()> {
    match command {
        VncserverCommand::Help => {
            print!("{USAGE}");
            Ok(())
        }
        VncserverCommand::List => {
            list();
            Ok(())
        }
        VncserverCommand::Kill(display) => kill(display),
        VncserverCommand::Start {
            display,
            geometry,
            listen,
        } => start(display, geometry, listen),
    }
}

/// TigerVNC's `vncserver` config (its config folder first, then `~/.vnc`),
/// or an empty string. Read only.
/// TigerVNC's administrator settings that override the user's
/// (`vncserver-config-mandatory`), or an empty string.
pub fn mandatory_config_file() -> String {
    std::fs::read_to_string("/etc/tigervnc/vncserver-config-mandatory").unwrap_or_default()
}

pub fn config_file() -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".config")));
    [
        config.map(|c| c.join("tigervnc").join("config")),
        home.map(|h| h.join(".vnc").join("config")),
    ]
    .into_iter()
    .flatten()
    .find_map(|path| std::fs::read_to_string(path).ok())
    .unwrap_or_default()
}

/// `$XDG_RUNTIME_DIR/farsight`, created private; refused unless it is a real
/// directory owned by us and closed to others.
fn state_dir() -> Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is not set")?
        .join("farsight");
    let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
    let meta = std::fs::symlink_metadata(&dir)?;
    if !meta.is_dir()
        || meta.uid() != rustix::process::geteuid().as_raw()
        || meta.mode() & 0o077 != 0
    {
        bail!("{} must be a private directory of yours", dir.display());
    }
    Ok(dir)
}

/// How [`private_file`] treats an existing file.
#[derive(Clone, Copy)]
enum Existing {
    Truncate,
    Refuse,
}

/// Opens `path` for writing, owner-only, never through a symlink.
fn private_file(path: &std::path::Path, existing: Existing) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};
    let mut flags = OFlags::WRONLY | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    flags |= match existing {
        Existing::Truncate => OFlags::TRUNC,
        Existing::Refuse => OFlags::EXCL,
    };
    let fd = rustix::fs::open(path, flags, Mode::from_raw_mode(0o600))?;
    Ok(std::fs::File::from(fd))
}

fn pid_file(display: u32) -> Result<PathBuf> {
    Ok(state_dir()?.join(format!("headless-{display}.pid")))
}

/// The agent process for `display`, if it is still running: the pidfile's
/// process must be this very program, serving that display (pids get
/// reused, e.g. by an unrelated `--headless` browser).
fn running(display: u32) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(pid_file(display).ok()?)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    if exe != std::env::current_exe().ok()? {
        return None;
    }
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<&[u8]> = cmdline.split(|&b| b == 0).collect();
    let wanted = format!(":{display}");
    args.windows(2)
        .any(|pair| matches!(pair, [flag, value] if *flag == b"--display" && *value == wanted.as_bytes()))
        .then_some(pid)
}

fn port_free(display: u32) -> bool {
    u16::try_from(5900 + display)
        .is_ok_and(|port| std::net::TcpListener::bind(("127.0.0.1", port)).is_ok())
}

fn start(display: Option<u32>, geometry: (u16, u16), listen: Listen) -> Result<()> {
    let (display, mut claim) = match display {
        Some(display) => (
            display,
            claim(display)?.context(format!("display :{display} is already in use"))?,
        ),
        None => (1..=99)
            .find_map(|d| claim(d).ok().flatten().map(|file| (d, file)))
            .context("no free display between :1 and :99")?,
    };
    let log = state_dir()?.join(format!("headless-{display}.log"));
    let log_file = private_file(&log, Existing::Truncate)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--backend", "headless", "--display", &format!(":{display}")])
        .args(["--geometry", &format!("{}x{}", geometry.0, geometry.1)])
        .stdin(Stdio::null())
        .stdout(log_file.try_clone()?)
        .stderr(log_file);
    if listen == Listen::Lan {
        command.args(["--listen", "lan"]);
    }
    // Its own process group, so it outlives this command and the terminal.
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let spawned = command.spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            let _ = std::fs::remove_file(pid_file(display)?);
            return Err(error).context("starting the headless desktop");
        }
    };
    std::io::Write::write_all(&mut claim, child.id().to_string().as_bytes())?;
    drop(claim);
    std::thread::sleep(STARTUP_CHECK);
    if let Some(status) = child.try_wait()? {
        // Our own pidfile (created exclusively above), so ours to remove.
        let _ = std::fs::remove_file(pid_file(display)?);
        let reason = std::fs::read_to_string(&log).unwrap_or_default();
        bail!("the desktop stopped ({status}):\n{}", reason.trim());
    }
    println!(
        "New Farsight desktop is :{display} (port {}). Log: {}",
        5900 + display,
        log.display()
    );
    Ok(())
}

/// Claims `display` by creating its pidfile exclusively (so two `vncserver`
/// runs can't both take it). A stale pidfile (its process gone) is replaced.
/// `None` when the display is in use.
fn claim(display: u32) -> Result<Option<std::fs::File>> {
    if running(display).is_some() || !port_free(display) {
        return Ok(None);
    }
    let path = pid_file(display)?;
    let open = || private_file(&path, Existing::Refuse);
    match open() {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Not running (checked above): stale. Another run may race us
            // here; create_new decides.
            std::fs::remove_file(&path)?;
            Ok(open().ok())
        }
        Err(error) => Err(error.into()),
    }
}

fn kill(display: u32) -> Result<()> {
    let Some(pid) = running(display) else {
        let _ = std::fs::remove_file(pid_file(display)?);
        bail!("no Farsight desktop is running on :{display}");
    };
    let pid = rustix::process::Pid::from_raw(pid).context("invalid process id")?;
    rustix::process::kill_process(pid, rustix::process::Signal::TERM)
        .context("stopping the desktop")?;
    // Wait for it to go, so `vncserver :N` right after works.
    for _ in 0..50 {
        if running(display).is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_file(pid_file(display)?);
    println!("Stopped the Farsight desktop on :{display}.");
    Ok(())
}

fn list() {
    println!("Farsight headless desktops:\n\nDISPLAY\tPROCESS ID");
    for display in 1..=99 {
        if let Some(pid) = running(display) {
            println!(":{display}\t{pid}");
        }
    }
}
