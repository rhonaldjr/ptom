# Roadmap

Every item here has passed the [roadmap council](../process/roadmap-council.md). The link in
each heading points to its session record. Requirement IDs refer to
[requirements.md](../spec/requirements.md). The order is set by the Product Owner, subject to
dependencies. Milestones are scope, not dates.

Legend: ☐ not started · ◐ in progress · ☑ done

---

## M0: Foundation ☑ · [council](../council/2026-10-23-initial-roadmap.md#m0-foundation)
- ☑ Cargo workspace, layered crates, workspace lints (`unsafe_code = forbid`)
- ☑ CLAUDE.md, personas, council process, ADRs 0001–0011
- ☑ CI gate (fmt, clippy, test, deny, layer check) and release-per-push workflow
- ☑ `cargo-fuzz` harness with targets for the version, pixel-format and security-selection code, plus a CI smoke run
- ⚠ **User action:** enable branch protection on `main` (require CI and one review). This is a repository setting, not code.

## M1: RFB core, sans-I/O · [council](../council/2026-10-23-initial-roadmap.md#m1-rfb-core)
NET-1, NET-3, NET-4, NET-5, NET-6, ENC-1, ENC-2, SEC-1, SEC-2, SEC-3
- Version negotiation 3.3/3.7/3.8, security handshake, ClientInit/ServerInit
- All core client and server messages, with bounded decoders
- Client and server state machines (phase-enforced: no input before auth)
- Raw and CopyRect
- None/VncAuth implemented but **gated**: loopback-only unless explicitly enabled, with warnings
- **Conditions:** fuzz target per decoder · proptest round-trips · recorded-transcript tests

## M2: Viewer MVP: view my existing VNC server · [council](../council/2026-10-23-initial-roadmap.md#m2-viewer-mvp)
VWR-1, VWR-2, VWR-3, VWR-5, VWR-6, VWR-10, VWR-14, INP-1, INP-2, INP-5, ENC-5 (decode), ENC-6 (decode)
- libcosmic viewer: connect, render, keyboard/mouse, clipboard, fullscreen/windowed, scaling, view-only
- Decoders: Raw, CopyRect, Hextile, ZRLE, Tight (incl. JPEG)
- TigerVNC-compatible CLI (`farsight host:display`, `-ViewOnly`, `-FullScreen`, …)
- **Conditions:** persistent encryption indicator · decoders fuzzed (malicious-server cases, API10)
  · TigerVNC server interop test in CI

## M3: Secure transport and vault · [council](../council/2026-10-23-initial-roadmap.md#m3-secure-transport--vault)
SEC-5, SEC-6, SEC-7, SEC-8, SEC-9, PRD-10, VWR-8, VWR-9, UTL-1
- VeNCrypt X509None/X509Vnc/X509Plain over rustls · RA2/RA2ne/RA2_256
- TOFU fingerprint pinning with a clear "identity changed" flow
- `farsight-vault` (ADR-0008) · connection history with search · `farsight-passwd`
- **Conditions:** downgrade test (MITM strips VeNCrypt, so the viewer warns or refuses) · secrets never logged (test)

## M3.5: Find any VNC server · [council](../council/2026-10-23-tray-and-discovery.md#b-discover-any-vnc-server-not-just-farsight-agents)
PRD-4, VWR-13, VWR-16, VWR-17
- "Computers on your network": passive `_rfb._tcp` browsing for **any** VNC server, always on
- Active probing (banner and security types only, never auth) that runs automatically on networks the
  user marked "This is my network". The consent is stored in the vault per network and can be cleared (ADR-0012).
- One-click **Add**, which creates a saved session without connecting. Entries show "Identity not verified" and an advisory "Not encrypted".
- **Conditions:** fuzzed probe parser reusing the `farsight-rfb` decoders · Q2 test proving no bytes are sent after the
  security list · Q4 interop containers (TigerVNC, x11vnc, QEMU, mDNS advertiser) · advertised servers appear within 5 s,
  and an opted-in /24 scan finishes within 30 s · scope limited to local subnets, rate-limited · credentials keyed by verified identity

## M4: COSMIC / Wayland live-session agent · [council](../council/2026-10-23-initial-roadmap.md#m4-cosmic--wayland-agent)
SRV-3, SRV-4, SRV-7, SRV-8, SRV-9, SRV-10, SRV-12, SEC-10, NET-2, NET-8 (view-only), ENC-1/2/5/6 (encode)
- `farsight-agent --backend wayland`: portal ScreenCast/RemoteDesktop with PipeWire and libei. Targets COSMIC first, then GNOME and KDE.
- systemd user unit with ADR-0009 hardening · loopback-only default · access controls and auth rate limiting
- Session-presence indicator plus "disconnect all" for the local user
- **Conditions:** requires M3 (no off-loopback agent without encryption) · use cases tested via
  fake ports in CI, plus a recorded Q3 exploratory charter on Pop!_OS COSMIC per release until M7
  provides a CI compositor · view-only enforcement test (API3)

## M5: X11 agent (x0vncserver parity) · [council](../council/2026-10-23-initial-roadmap.md#m5-x11-agent)
SRV-2, SRV-7 (XTest), SRV-10 (XDamage + polling)
- `farsight-agent --backend x11` with an Xvfb-based integration test in CI

## M6: Multi-session viewer, tray icon and agent advertising · [council](../council/2026-10-23-initial-roadmap.md#m6-multi-session--discovery) · [tray council](../council/2026-10-23-tray-and-discovery.md#a-viewer-tray-icon)
PRD-3, VWR-12, VWR-15
- Tabs and windows, with a tab dragged out into a new window · per-session status
- Agent-side mDNS **advertising** (ADR-0010). Browsing moved to M3.5.
- **Tray icon** (ADR-0013), in slices: saved sessions + Quit → raise open session → discovered-servers submenu → optional close-to-tray (off by default)
- **Conditions:** minimal TXT records · advertise only when off-loopback · rogue-agent spoof test ·
  tray CI tests with a stub SNI watcher and with no watcher · Q3 charter per DE in the support matrix ·
  tray shows labels only, "Unlock…" while the vault is locked · names sanitised · unverified entries never auto-send credentials

## M7: Persistent headless desktops · [council](../council/2026-10-23-initial-roadmap.md#m7-headless-desktops)
SRV-1, SRV-5, VWR-4, NET-8 (shared)
- `farsight-headless` Smithay compositor plus Xwayland (ADR-0006) · ExtendedDesktopSize · multi-monitor layouts · shared sessions
- socket activation · `farsight-agent@:N` units mirroring `vncserver@:N`
- Becomes the CI compositor for end-to-end tests

## M8: Performance and full encodings · [council](../council/2026-10-23-initial-roadmap.md#m8-performance--encodings)
ENC-3, ENC-4, ENC-6, ENC-7, ENC-9, ENC-10, ENC-11, NET-7, SRV-6, PLT-4
- Server encoders for the full set · auto-selection · quality/compression controls
- ContinuousUpdates, Fence, congestion control · remaining pseudo-encodings
- **Conditions:** `criterion` benchmarks and published latency/fps baselines · zlib-bomb limits

## M9: Compatibility and polish · [council](../council/2026-10-23-initial-roadmap.md#m9-compatibility--polish)
PLT-3, INP-3, INP-4, VWR-7, VWR-11, UTL-2, UTL-3, UTL-4
- Interop matrix in CI: TigerVNC, TightVNC, RealVNC (viewer side), and noVNC
- Compat names (`vncviewer`, `vncserver`, `x0vncserver`, `w0vncserver`, `vncpasswd`), installed as **opt-in** symlinks
- System-key capture per DE · LED sync · QEMU key events · runtime config reload · reading legacy `~/.vnc` config
- KDE and Xfce certification rows · macOS viewer polish

## M10: GPU paths and H.264 · [council](../council/2026-10-23-initial-roadmap.md#m10-gpu--h264)
SRV-11, ENC-8
- DMA-BUF capture, VA-API/Vulkan encode, and H.264 behind an **optional** cargo feature
- **Conditions:** licensing review before enabling in release builds

---

## Horizon (deferred, needs its own council session before scheduling)
- **Web interface over Tailscale** (PRD-9): WebSocket gateway, authenticated, SSRF-safe
- Tight security type (SEC-4)
- Privilege-separated agent (protocol process / capture process)
- Greeter / login-screen access (needs a privileged, audited component)
- Vault export / backup
