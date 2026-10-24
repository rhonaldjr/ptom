# Q3 charter: viewer tray icon, tabs and windows (M6)

*Explore* the viewer's tray icon and multi-session handling *with* each desktop in the
[ADR-0013 support matrix](../../adr/0013-viewer-tray-icon.md) *to discover* where the tray or tabs fail
to appear, mislead, or lose input.

## Setup
Two reachable VNC servers: a Farsight agent (`farsight-agent --listen lan`) and a TigerVNC server.
Run on Pop!_OS COSMIC, Pop!_OS GNOME, vanilla GNOME (Fedora/Ubuntu without AppIndicator), KDE Plasma,
Xfce (with and without the StatusNotifier plugin), and macOS.

## Checks
1. **Tray presence:** the icon appears on COSMIC, KDE, Pop!_OS GNOME and Xfce with the plugin. On vanilla
   GNOME there is no icon, no error dialog, and everything is reachable from the window (Home tab).
2. **Tray menu:** it lists "Show Farsight", open sessions, saved computers and computers on the network
   ("not verified"), plus Quit. Labels contain no odd characters (sanitised) and no usernames.
3. **Locked vault** (start without a keyring): the tray shows "Unlock saved computers…" and no saved names.
4. **Tooltip:** it says when an open session is NOT encrypted.
5. **Tabs:** connect to both servers. Each opens in a new tab (U-7). Switching tabs never leaves a key held
   on the other computer (hold Shift, switch, release, check the old session).
6. **Move to new window** (F8 menu): input goes to the right session, and closing that window disconnects it.
7. **Full screen** in a tab hides the tab bar. Leaving full screen restores it.
8. **Remote resize:** resizing the main window resizes only the active tab's desktop.

Record findings and bugs here when this charter is run.
