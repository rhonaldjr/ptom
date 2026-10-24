//! `farsight-agent`: the VNC server. Composition root only; capture backends
//! arrive in M4 (Wayland), M5 (X11) and M7 (headless).

use anyhow::bail;

fn main() -> anyhow::Result<()> {
    if rustix::process::geteuid().is_root() {
        bail!("farsight-agent must not run as root; run it as the desktop user (ADR-0009)");
    }
    println!(
        "farsight-agent {}: server not implemented yet (roadmap M4)",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
