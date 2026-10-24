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

## M1: RFB core, sans-I/O ☑ · [council](../council/2026-10-23-initial-roadmap.md#m1-rfb-core)
NET-1, NET-3, NET-4, NET-5, NET-6, ENC-1, ENC-2, SEC-1, SEC-2, SEC-3
- ☑ Version negotiation 3.3/3.7/3.8, security handshake, ClientInit/ServerInit
- ☑ All core client and server messages, with bounded decoders
- ☑ Client and server state machines (phase-enforced: no input before auth) ([ADR-0015](../adr/0015-sans-io-connections.md))
- ☑ Raw and CopyRect (decode and encode), plus the DesktopSize and LastRect pseudo-encodings
- ☑ None/VncAuth implemented but **gated** by `SecurityPolicy`: loopback-only unless explicitly enabled
- ☑ **Conditions:** fuzz target per decoder and state machine · proptest round-trips · RFC 6143 transcript test
  (a transcript recorded from a real TigerVNC server is part of M2's interop condition)

## M2: Viewer MVP: view my existing VNC server ☑ · [council](../council/2026-10-23-initial-roadmap.md#m2-viewer-mvp)
VWR-1, VWR-2, VWR-3, VWR-5, VWR-6, VWR-10, VWR-14, INP-1, INP-2, INP-5, ENC-5 (decode), ENC-6 (decode)
- ☑ libcosmic viewer: connect, render, keyboard/mouse, clipboard, fullscreen/windowed, scaling, view-only
- ☑ Decoders: Raw, CopyRect, Hextile, ZRLE, Tight (incl. JPEG)
- ☑ Pseudo-encodings: ExtendedDesktopSize/SetDesktopSize (remote resize) and Cursor/CursorWithAlpha, decode only ([scope council](../council/2026-10-23-m2-scope.md))
- ☑ TigerVNC-compatible CLI (`farsight host:display`, `-ViewOnly`, `-FullScreen`, `-RemoteResize`, …)
- ☑ **Conditions:** persistent encryption indicator (header, and the F8 menu when full screen) · decoders fuzzed
  (malicious-server cases, API10) · TigerVNC server interop test in CI (`tests/interop/tigervnc`)
- Not yet verified by hand: the macOS build (tier 2), and clipboard plus keyboard against a full desktop session. These are Q3 charters for the next release.

## M3: Secure transport and vault ☑ · [council](../council/2026-10-23-initial-roadmap.md#m3-secure-transport--vault)
SEC-5, SEC-6, SEC-7, SEC-8, SEC-9, PRD-10, VWR-8, VWR-9, UTL-1
- ☑ VeNCrypt X509None/X509Vnc/X509Plain over rustls · RA2/RA2_256 via aws-lc-rs ([ADR-0016](../adr/0016-ra2-crypto-aws-lc.md)); the "ne" variants are not supported
- ☑ TOFU fingerprint pinning with a clear "identity changed" flow ([ADR-0017](../adr/0017-server-identity-verification.md))
- ☑ `farsight-vault` (ADR-0008) · connection history with search · `farsight-passwd` (ADR-0014)
- ☑ **Conditions:** downgrade test (a pinned server offering only legacy security is refused) · secrets never logged (test)
- Interop verified in CI against TigerVNC: VncAuth, None, X509Vnc, RA2 and RA2_256
- Follow-ups: manage saved computers (rename, forget) in the UI. The passphrase unlock screen moved to M3.5 by user decision (P-8).

## M3.5: Find any VNC server ☑ · [council](../council/2026-10-23-tray-and-discovery.md#b-discover-any-vnc-server-not-just-farsight-agents)
PRD-4, VWR-13, VWR-16, VWR-17
- ☑ "Computers on your network": passive `_rfb._tcp` browsing for **any** VNC server, always on
- ☑ Active probing (banner and security types only, never auth) that runs automatically on networks the
  user marked "This is my network". The consent is stored in the vault per network and can be cleared (ADR-0012).
- ☑ **Passphrase unlock** for saved computers when the desktop has no keyring (user decision on P-8, ADR-0008)
- ☑ One-click **Add**, which creates a saved session without connecting. Entries show "Identity not verified" and an advisory "Not encrypted".
- ☑ **Conditions:** fuzzed probe parser reusing the `farsight-rfb` decoders · Q2 test proving no bytes are sent after the
  security list · Q4 interop containers (TigerVNC, x11vnc, QEMU, mDNS advertiser) · advertised servers appear within 5 s,
  and an opted-in /24 scan finishes within 30 s · scope limited to local subnets, rate-limited · credentials keyed by verified identity

## M4: COSMIC / Wayland live-session agent ☑ · [council](../council/2026-10-23-initial-roadmap.md#m4-cosmic--wayland-agent) · [ADR-0018](../adr/0018-agent-architecture.md)
SRV-3, SRV-4, SRV-7, SRV-8, SRV-9, SRV-10, SRV-12, SEC-10, NET-2, NET-8 (view-only), ENC-1/2/5/6 (encode)
- ☑ `farsight-agent --backend wayland`: xdg-desktop-portal RemoteDesktop/ScreenCast + PipeWire; approval remembered with a restore token (U-3)
- ☑ `farsight-server`: per-viewer sessions, shared desktop with damage tracking, Raw/ZRLE/Tight (no JPEG) encoders, access control and lockout
- ☑ systemd user unit with ADR-0009 hardening, shipped disabled (U-6) · loopback-only default (U-5) · TLS and RA2 identity created on first run
- ☑ Approval prompt for anyone but the owner (U-2) · presence tray icon and notifications with "Disconnect everyone" (U-4)
- ☑ **Conditions:** requires M3 ✓ · use cases tested with fake ports in CI ✓ · view-only enforced by the server (test) ✓ ·
  Q3 charter for live portals: [docs/testing/charters/2026-10-24-wayland-capture.md](../testing/charters/2026-10-24-wayland-capture.md)
- Not yet verified on a live Wayland desktop (the build machine runs X11). Clipboard over the Wayland portal is still to come.

## M5: X11 agent (x0vncserver parity) ☑ · [council](../council/2026-10-23-initial-roadmap.md#m5-x11-agent)
SRV-2, SRV-7 (XTest), SRV-10 (XDamage + polling)
- ☑ `farsight-agent --backend x11` (`farsight-capture-x11`, pure-Rust x11rb): XDamage with MIT-SHM or GetImage, polling fallback,
  RandR resizes, XFixes cursor, XTest input with on-demand keysym remapping, CLIPBOARD both ways
- ☑ Integration tests against Xvfb, plus an end-to-end CI job: agent on Xvfb ↔ Farsight viewer session, encrypted, owner login, pixels checked
- ☑ **Condition:** X11 can't isolate clients from each other (any X client can inject input or read the screen); recorded in the threat model

## M6: Multi-session viewer, tray icon and agent advertising ☑ · [council](../council/2026-10-23-initial-roadmap.md#m6-multi-session--discovery) · [tray council](../council/2026-10-23-tray-and-discovery.md#a-viewer-tray-icon)
PRD-3, VWR-12, VWR-15
- ☑ Tabs (a new connection opens in a tab, U-7) and windows ("Move to new window", P-29) · per-session status and input routing
- ☑ Agent-side mDNS **advertising** (ADR-0010) with `--listen lan` (P-31). Browsing is in M3.5.
- ☑ **Tray icon** (ADR-0013): Show, open sessions, saved computers, discovered computers (unverified), Quit; "Unlock…" while the vault is locked.
  Close-to-tray is deferred (P-28).
- ☑ **Conditions:** minimal TXT records ✓ · advertise only when off-loopback ✓ · rogue-agent spoof test ✓ ·
  tray CI test on a private D-Bus with and without a host ✓ · Q3 charter per desktop:
  [docs/testing/charters/2026-10-24-tray-and-tabs.md](../testing/charters/2026-10-24-tray-and-tabs.md) ·
  labels only, sanitised ✓ · unverified entries never auto-send credentials (credentials are bound to address and identity) ✓
- Not yet verified by hand on a live desktop (Q3 charter pending).

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
