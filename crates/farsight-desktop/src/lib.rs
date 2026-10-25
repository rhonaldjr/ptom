//! Desktop integration for the agent and viewer: notifications, the
//! "someone wants to connect" prompt, and `StatusNotifierItem` tray icons
//! (ADR-0013). Linux uses D-Bus (pure-Rust zbus via `notify-rust` and `ksni`);
//! other platforms get no-op fallbacks for now.
//!
//! Layer: **Adapters** (ADR-0002).

#[cfg(target_os = "linux")]
mod icons;
pub mod notify;
pub mod presence;
pub mod tray;
