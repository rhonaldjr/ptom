# ADR-0013: Viewer tray icon via StatusNotifierItem (ksni) and a macOS status item

- **Status:** Accepted (council) · **Date:** 2026-10-23
- **Council:** [tray and discovery](../council/2026-10-23-tray-and-discovery.md#a-viewer-tray-icon)
- **Requirements:** VWR-15

## Decision
- `ptom-client` owns a `StatusPresenter` port and a status view model listing saved sessions, open sessions,
  discovered servers and their state. The main window and the tray render the **same** model. Every
  tray action also exists in the main window.
- Linux adapter: **`ksni`** (pure-Rust StatusNotifierItem over D-Bus). It does not use `tray-icon`, which
  pulls in GTK/libappindicator. Works on COSMIC, KDE Plasma, Pop!_OS GNOME (AppIndicator extension) and Xfce
  (SNI plugin).
- macOS adapter: NSStatusItem, in its own crate with an FFI ADR (the workspace forbids `unsafe`). It ships
  after the Linux adapter.
- Tray availability is an explicit state (`Available` / `NoHost`). With no host (e.g. vanilla GNOME), a null
  adapter is used, everything still works from the main window, and no errors are shown.
- Close-to-tray is **off by default**. Enabling it shows a one-time notice.
- Privacy and security: the tray shows labels only (no usernames). While the vault is locked it shows only
  "Unlock…". Discovered servers can be hidden from the tray. Advertised names are sanitised (control and
  bidi characters stripped, length capped). Unverified servers are marked, and connecting to them goes
  through TOFU without sending saved credentials.

## Support matrix (acceptance criteria)
| DE | Tray host | Expectation |
|---|---|---|
| COSMIC | status-area applet (SNI) | icon + menu |
| KDE Plasma | native SNI | icon + menu |
| Pop!_OS GNOME | AppIndicator extension (preinstalled) | icon + menu |
| Vanilla GNOME | none | no icon, no errors, full main-window parity |
| Xfce | SNI plugin | icon + menu (when the plugin is present) |
| macOS | NSStatusItem | menu-bar icon (tier 2) |
