# farsight Requirements (baseline 2026-10-23)

This is the original brief, normalised and given IDs so that roadmap items, stories and tests can
reference it. Clarifications from the [spec review](spec-review.md) have been applied, and items
removed or changed by those decisions are struck through or annotated.
Each ID maps to a roadmap milestone in [ROADMAP.md](../roadmap/ROADMAP.md).

## Product (PRD)
- **PRD-1** Open-source (MIT), Rust, drop-in replacement for TigerVNC (CLI flags, `~/.vnc` / XDG config, systemd units, wire interop).
- **PRD-2** Works on X11 and Wayland.
- **PRD-3** Viewer stores multiple desktop sessions and opens them in tabs or separate windows.
- **PRD-4** Viewer automatically discovers VNC servers on the home network (LAN/Wi-Fi): Farsight agents **and any other RFB server**. It lists them and offers to add them ([ADR-0012](../adr/0012-active-rfb-probing.md)).
- **PRD-5** Desktop environments, in priority order: COSMIC (Pop!_OS, Rust COSMIC epoch), GNOME (incl. Pop!_OS GNOME), KDE Plasma, Xfce, others.
- **PRD-6** Comprehensive automated testing: unit, property, fuzz, integration and interop.
- **PRD-7** GitHub Actions cuts a versioned release on every push to `main` ([ADR-0005](../adr/0005-release-per-push.md)).
- **PRD-8** Security first. Agents run with the least privilege possible.
- **PRD-9** *(future)* Web interface reachable via Tailscale or similar. The architecture must not preclude it.
- **PRD-10** Client-side information about each remote machine (host, credentials, fingerprints, settings) is stored encrypted.

## Core protocol and networking (NET)
- **NET-1** RFB 3.3, 3.7 and 3.8, with version negotiation (unknown 3.x treated per RFC 6143).
- **NET-2** TCP listener (server) and TCP connect (client).
- **NET-3** Pixel format negotiation: bpp/depth, true colour, colour maps.
- **NET-4** FramebufferUpdateRequest, both full and incremental.
- **NET-5** All core messages: SetPixelFormat, SetEncodings, FramebufferUpdateRequest, KeyEvent, PointerEvent, ClientCutText, FramebufferUpdate, ServerCutText, Bell, SetColourMapEntries.
- **NET-6** Explicit client and server state machines (sans-I/O).
- **NET-7** Congestion control and update throttling.
- **NET-8** Shared sessions (multiple clients) and view-only clients.

## Encodings (ENC)
- **ENC-1** Raw · **ENC-2** CopyRect · **ENC-3** RRE · **ENC-4** Hextile · **ENC-5** ZRLE
- **ENC-6** Tight (zlib and JPEG sub-encodings; SIMD JPEG). This is the preferred encoding.
- **ENC-7** Standalone JPEG · **ENC-8** H.264 (optional, hardware-accelerated where available)
- **ENC-9** Automatic encoding selection based on bandwidth, latency and CPU
- **ENC-10** Quality and compression level controls
- **ENC-11** Pseudo-encodings: DesktopSize, ExtendedDesktopSize, Cursor, CursorWithAlpha, ContinuousUpdates, Fence, ExtendedClipboard, QEMU Extended Key Event, LED State, Extended Mouse Buttons

## Security and authentication (SEC)
- **SEC-1** Security-type negotiation, with downgrade resistance where the protocol allows it
- **SEC-2** None. Legacy, opt-in, loopback-only by default.
- **SEC-3** VncAuth (DES challenge-response). Legacy, opt-in off-loopback, and warns when used.
- **SEC-4** Tight security type *(deferred to the Horizon list by council)*
- **SEC-5** VeNCrypt: X509None, X509Vnc, X509Plain, and Plain over TLS with an authenticated server
- **SEC-5a** VeNCrypt **anonymous** TLS (TLSNone/TLSVnc/TLSPlain). **Not supported:** rustls has no anonymous DH ([ADR-0007](../adr/0007-crypto-stack.md)).
- **SEC-6** RSA-AES (RA2, RA2ne, RA2_256) with server key fingerprint pinning (TOFU)
- **SEC-7** Password file compatible with `vncpasswd`
- **SEC-8** Session TLS via rustls
- **SEC-9** Server identity verification: X.509 chains, pinned self-signed certificates, RA2 fingerprints
- **SEC-10** Access controls: localhost-only, accept/reject prompt (defaults to **reject** on timeout), IP allow/deny lists, connection limits, and auth rate limiting/lockout

## Server / agent (SRV)
- **SRV-1** Headless persistent desktops (Xvnc equivalent) using a Smithay headless compositor plus Xwayland ([ADR-0006](../adr/0006-headless-smithay.md))
- **SRV-2** Share an existing X11 display (x0vncserver equivalent) with XDamage and a polling fallback
- **SRV-3** Share an existing Wayland session (w0vncserver equivalent): xdg-desktop-portal ScreenCast/RemoteDesktop with PipeWire (GNOME, KDE, COSMIC), plus COSMIC/wlroots protocols where they are available
- **SRV-4** systemd integration: `farsight-agent@.service` **user** units, socket activation
- **SRV-5** Desktop resizing (ExtendedDesktopSize / SetDesktopSize) and multi-monitor layout
- **SRV-6** Continuous updates mode
- **SRV-7** Input injection: keyboard, mouse, extra buttons (XTest on X11; libei/portal RemoteDesktop on Wayland)
- **SRV-8** Clipboard redirection, standard and extended
- **SRV-9** Server-side cursor shape handling
- **SRV-10** Change detection: XDamage, Wayland damage/PipeWire, polling fallback. ~~Windows hooking~~ (out of scope, [ADR-0004](../adr/0004-platform-scope.md)).
- **SRV-11** GPU paths: DMA-BUF/GBM capture, VA-API/Vulkan encode where supported
- **SRV-12** A session-presence indicator visible to the local user (added by the council for Privacy by Design)

## Viewer (VWR)
- **VWR-1** Native viewer built on libcosmic. Linux is tier 1, macOS is tier 2. ~~FLTK~~, ~~Windows~~.
- **VWR-2** Fullscreen and windowed · **VWR-3** Remote resize and local scaling
- **VWR-4** Multi-monitor fullscreen · **VWR-5** Bidirectional clipboard
- **VWR-6** Configurable viewer shortcuts (fullscreen toggle, release grab, menu key)
- **VWR-7** System-key capture and forwarding on macOS, GNOME, COSMIC and KDE (Wayland keyboard-shortcuts-inhibit protocol or portal)
- **VWR-8** Connection history with search
- **VWR-9** Remember credentials, stored only through the encrypted vault (PRD-10)
- **VWR-10** View-only mode · **VWR-11** Encoding preference selection
- **VWR-12** Sessions in tabs and/or windows (PRD-3) · **VWR-13** Discovered-agents list (PRD-4)
- **VWR-14** Encryption/identity status always visible per session
- **VWR-15** Tray / status-area icon on COSMIC, KDE, Pop!_OS GNOME and Xfce (SNI), and macOS (menu bar), with full main-window parity and graceful fallback when the desktop has no tray host ([ADR-0013](../adr/0013-viewer-tray-icon.md))
- **VWR-16** Discover any RFB server: passive mDNS always on, plus active probing on networks the user has marked as their own ([ADR-0012](../adr/0012-active-rfb-probing.md))
- **VWR-17** One-click "Add" of a discovered server as a saved session (shown as unverified until TOFU)

## Input (INP)
- **INP-1** Keyboard events with keymap remapping · **INP-2** Mouse incl. back/forward buttons
- **INP-3** LED state sync · **INP-4** QEMU extended key events · **INP-5** View-only / ignore input

## Utilities (UTL)
- **UTL-1** Password management (`farsight-passwd`, vault CLI) · **UTL-2** Runtime configuration (reload without restart)
- **UTL-3** XDG Base Directory layout, plus reading legacy `~/.vnc/` · **UTL-4** Session-management and auth-setup tools

## Platform (PLT)
- **PLT-1** Linux/Unix first (virtual, physical X11 and Wayland)
- **PLT-2** Viewer on Linux and macOS
- **PLT-3** Interop with TigerVNC, TightVNC, RealVNC and noVNC, verified in CI
- **PLT-4** Performance for video, 3D and fast-changing content, with published benchmarks
