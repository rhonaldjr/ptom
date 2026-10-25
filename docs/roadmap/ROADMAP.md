# Roadmap

Every item here has passed the [roadmap council](../process/roadmap-council.md). The link in
each heading points to its session record. Requirement IDs refer to
[requirements.md](../spec/requirements.md). The order is set by the Product Owner, subject to
dependencies. Milestones are scope, not dates.

## Status at a glance

| Milestone | Status |
|---|---|
| M0 Foundation | ✅ Done |
| M1 RFB core, sans-I/O | ✅ Done |
| M2 Viewer MVP | ✅ Done |
| M3 Secure transport and vault | ✅ Done |
| M3.5 Find any VNC server | ✅ Done |
| M4 COSMIC / Wayland agent | ✅ Done (live-desktop check pending) |
| M5 X11 agent | ✅ Done |
| M6 Multi-session viewer, tray, advertising | ✅ Done (hand check pending) |
| M7 Persistent headless desktops | ✅ Done |
| M8 Performance and full encodings | ✅ Done |
| M9 Compatibility and polish | ✅ Done (hand checks pending) |
| M10 GPU paths and H.264 | ⬜ Not started |

Each item below is a checkbox: ticked means built, tested and committed.

---

## M0: Foundation ✅ Done · [council](../council/2026-10-23-initial-roadmap.md#m0-foundation)
- [x] Cargo workspace, layered crates, workspace lints (`unsafe_code = forbid`)
- [x] CLAUDE.md, personas, council process, ADRs 0001–0011
- [x] CI gate (fmt, clippy, test, deny, layer check) and release-per-push workflow
- [x] `.deb` packages in every release: `farsight` (viewer) and `farsight-agent` (agent, `farsight-passwd`, systemd user unit), amd64 and arm64 (user request, P-35)
- [x] `cargo-fuzz` harness with targets for the version, pixel-format and security-selection code, plus a CI smoke run
- ⚠ **User action:** enable branch protection on `main` (require CI and one review). This is a repository setting, not code.

## M1: RFB core, sans-I/O ✅ Done · [council](../council/2026-10-23-initial-roadmap.md#m1-rfb-core)
NET-1, NET-3, NET-4, NET-5, NET-6, ENC-1, ENC-2, SEC-1, SEC-2, SEC-3
- [x] Version negotiation 3.3/3.7/3.8, security handshake, ClientInit/ServerInit
- [x] All core client and server messages, with bounded decoders
- [x] Client and server state machines (phase-enforced: no input before auth) ([ADR-0015](../adr/0015-sans-io-connections.md))
- [x] Raw and CopyRect (decode and encode), plus the DesktopSize and LastRect pseudo-encodings
- [x] None/VncAuth implemented but **gated** by `SecurityPolicy`: loopback-only unless explicitly enabled
- [x] **Conditions:** fuzz target per decoder and state machine · proptest round-trips · RFC 6143 transcript test
  (a transcript recorded from a real TigerVNC server is part of M2's interop condition)

## M2: Viewer MVP: view my existing VNC server ✅ Done · [council](../council/2026-10-23-initial-roadmap.md#m2-viewer-mvp)
VWR-1, VWR-2, VWR-3, VWR-5, VWR-6, VWR-10, VWR-14, INP-1, INP-2, INP-5, ENC-5 (decode), ENC-6 (decode)
- [x] libcosmic viewer: connect, render, keyboard/mouse, clipboard, fullscreen/windowed, scaling, view-only
- [x] Decoders: Raw, CopyRect, Hextile, ZRLE, Tight (incl. JPEG)
- [x] Pseudo-encodings: ExtendedDesktopSize/SetDesktopSize (remote resize) and Cursor/CursorWithAlpha, decode only ([scope council](../council/2026-10-23-m2-scope.md))
- [x] TigerVNC-compatible CLI (`farsight host:display`, `-ViewOnly`, `-FullScreen`, `-RemoteResize`, …)
- [x] **Conditions:** persistent encryption indicator (header, and the F8 menu when full screen) · decoders fuzzed
  (malicious-server cases, API10) · TigerVNC server interop test in CI (`tests/interop/tigervnc`)
- ⚠️ Not yet verified by hand: the macOS build (tier 2), and clipboard plus keyboard against a full desktop session. These are Q3 charters for the next release.

## M3: Secure transport and vault ✅ Done · [council](../council/2026-10-23-initial-roadmap.md#m3-secure-transport--vault)
SEC-5, SEC-6, SEC-7, SEC-8, SEC-9, PRD-10, VWR-8, VWR-9, UTL-1
- [x] VeNCrypt X509None/X509Vnc/X509Plain over rustls · RA2/RA2_256 via aws-lc-rs ([ADR-0016](../adr/0016-ra2-crypto-aws-lc.md)); the "ne" variants are not supported
- [x] TOFU fingerprint pinning with a clear "identity changed" flow ([ADR-0017](../adr/0017-server-identity-verification.md))
- [x] `farsight-vault` (ADR-0008) · connection history with search · `farsight-passwd` (ADR-0014)
- [x] **Conditions:** downgrade test (a pinned server offering only legacy security is refused) · secrets never logged (test)
- Interop verified in CI against TigerVNC: VncAuth, None, X509Vnc, RA2 and RA2_256
- Follow-ups: manage saved computers (rename, forget) in the UI. The passphrase unlock screen moved to M3.5 by user decision (P-8).

## M3.5: Find any VNC server ✅ Done · [council](../council/2026-10-23-tray-and-discovery.md#b-discover-any-vnc-server-not-just-farsight-agents)
PRD-4, VWR-13, VWR-16, VWR-17
- [x] "Computers on your network": passive `_rfb._tcp` browsing for **any** VNC server, always on
- [x] Active probing (banner and security types only, never auth) that runs automatically on networks the
  user marked "This is my network". The consent is stored in the vault per network and can be cleared (ADR-0012).
- [x] **Passphrase unlock** for saved computers when the desktop has no keyring (user decision on P-8, ADR-0008)
- [x] One-click **Add**, which creates a saved session without connecting. Entries show "Identity not verified" and an advisory "Not encrypted".
- [x] **Conditions:** fuzzed probe parser reusing the `farsight-rfb` decoders · Q2 test proving no bytes are sent after the
  security list · Q4 interop containers (TigerVNC, x11vnc, QEMU, mDNS advertiser) · advertised servers appear within 5 s,
  and an opted-in /24 scan finishes within 30 s · scope limited to local subnets, rate-limited · credentials keyed by verified identity

## M4: COSMIC / Wayland live-session agent ✅ Done · [council](../council/2026-10-23-initial-roadmap.md#m4-cosmic--wayland-agent) · [ADR-0018](../adr/0018-agent-architecture.md)
SRV-3, SRV-4, SRV-7, SRV-8, SRV-9, SRV-10, SRV-12, SEC-10, NET-2, NET-8 (view-only), ENC-1/2/5/6 (encode)
- [x] `farsight-agent --backend wayland`: xdg-desktop-portal RemoteDesktop/ScreenCast + PipeWire; approval remembered with a restore token (U-3)
- [x] `farsight-server`: per-viewer sessions, shared desktop with damage tracking, Raw/ZRLE/Tight (no JPEG) encoders, access control and lockout
- [x] systemd user unit with ADR-0009 hardening, shipped disabled (U-6) · loopback-only default (U-5) · TLS and RA2 identity created on first run
- [x] Approval prompt for anyone but the owner (U-2) · presence tray icon and notifications with "Disconnect everyone" (U-4)
- [x] **Conditions:** requires M3 ✓ · use cases tested with fake ports in CI ✓ · view-only enforced by the server (test) ✓ ·
  Q3 charter for live portals: [docs/testing/charters/2026-10-24-wayland-capture.md](../testing/charters/2026-10-24-wayland-capture.md)
- ⚠️ Not yet verified on a live Wayland desktop (the build machine runs X11). Clipboard over the Wayland portal is still to come.

## M5: X11 agent (x0vncserver parity) ✅ Done · [council](../council/2026-10-23-initial-roadmap.md#m5-x11-agent)
SRV-2, SRV-7 (XTest), SRV-10 (XDamage + polling)
- [x] `farsight-agent --backend x11` (`farsight-capture-x11`, pure-Rust x11rb): XDamage with MIT-SHM or GetImage, polling fallback,
  RandR resizes, XFixes cursor, XTest input with on-demand keysym remapping, CLIPBOARD both ways
- [x] Integration tests against Xvfb, plus an end-to-end CI job: agent on Xvfb ↔ Farsight viewer session, encrypted, owner login, pixels checked
- [x] **Condition:** X11 can't isolate clients from each other (any X client can inject input or read the screen); recorded in the threat model

## M6: Multi-session viewer, tray icon and agent advertising ✅ Done · [council](../council/2026-10-23-initial-roadmap.md#m6-multi-session--discovery) · [tray council](../council/2026-10-23-tray-and-discovery.md#a-viewer-tray-icon)
PRD-3, VWR-12, VWR-15
- [x] Tabs (a new connection opens in a tab, U-7) and windows ("Move to new window", P-29) · per-session status and input routing
- [x] Agent-side mDNS **advertising** with `--listen lan`, **only on networks you marked as yours** (`--trust-this-network`; [ADR-0019](../adr/0019-advertise-on-trusted-networks.md), user decision P-32). Browsing is in M3.5.
- [x] **Tray icon** (ADR-0013): Show, open sessions, saved computers, discovered computers (unverified), Quit; "Unlock…" while the vault is locked.
  Close-to-tray is deferred (P-28).
- [x] **Conditions:** minimal TXT records ✓ · advertise only when off-loopback ✓ · rogue-agent spoof test ✓ ·
  tray CI test on a private D-Bus with and without a host ✓ · Q3 charter per desktop:
  [docs/testing/charters/2026-10-24-tray-and-tabs.md](../testing/charters/2026-10-24-tray-and-tabs.md) ·
  labels only, sanitised ✓ · unverified entries never auto-send credentials (credentials are bound to address and identity) ✓
- ⚠️ Not yet verified by hand on a live desktop (Q3 charter pending).

## M7: Persistent headless desktops ✅ Done · [council](../council/2026-10-23-initial-roadmap.md#m7-headless-desktops) · [ADR-0020](../adr/0020-headless-desktops.md)
SRV-1, SRV-5, VWR-4, NET-8 (shared)
- [x] `farsight-headless` Smithay compositor plus Xwayland (ADR-0006): software rendering, startup script (U-10), clipboard both ways
- [x] ExtendedDesktopSize resizing · multi-monitor layouts (validated, up to 16 screens)
- [x] Shared sessions: one controller at a time, hand-over when idle, the owner can always take over (U-9, P-38)
- [x] socket activation · `farsight-headless@<port>` units mirroring `vncserver@:N` (P-37)
- [x] Becomes the CI compositor for end-to-end tests (`tests/e2e/agent-headless.sh`, CI job `headless`)
- [x] **Conditions:** slice single monitor → resize → multi-monitor ✓ · in-session indicator when others join (P-40) ✓ ·
  explicit shared-input policy (U-9) ✓
- ⚠️ X11 apps need Xwayland 23.1+ (P-36). Not yet used by hand on a real desktop.

## M8: Performance and full encodings ✅ Done · [council](../council/2026-10-23-initial-roadmap.md#m8-performance--encodings) · [baselines](../performance/baselines.md)
ENC-3, ENC-4, ENC-6, ENC-7, ENC-9, ENC-10, ENC-11, NET-7, SRV-6, PLT-4
- [x] Server encoders for the full set: Raw, RRE, Hextile, ZRLE, Tight with JPEG (the viewer decodes RRE too) (P-42)
- [x] Auto-selection per rectangle (solid → fill, photo → JPEG, else lossless) · JPEG quality and compression levels honoured · viewer `-NoJPEG` (P-43)
- [x] ContinuousUpdates and Fence on both sides · congestion control: at most two unacknowledged updates (P-44)
- [x] Remaining pseudo-encodings: DesktopName (QEMU keys and LED state are M9, P-45)
- [x] **Conditions:** `criterion` benchmarks, [published baselines](../performance/baselines.md), benchmarks attached to every release ✓ ·
  zlib-bomb limits (bounded inflate since M2, fuzzed) ✓

## M9: Compatibility and polish ✅ Done · [council](../council/2026-10-23-initial-roadmap.md#m9-compatibility--polish) · [matrix](../testing/compatibility-matrix.md)
PLT-3, INP-3, INP-4, VWR-7, VWR-11, UTL-2, UTL-3, UTL-4
- [x] Interop matrix in CI: TigerVNC 1.12/1.15 and TightVNC servers, and TigerVNC's viewer against the agent.
  RealVNC (proprietary) and noVNC (needs the web gateway) are hand checks / later (P-48)
- [x] Compat names (`vncviewer`, `vncserver`, `x0vncserver`, `w0vncserver`, `vncpasswd`) in the **opt-in**
  `farsight-tigervnc-compat` package, accepting TigerVNC's options (U-11, P-47)
- [x] QEMU key events (physical keys, any layout) · LED sync (remote Caps/Num Lock shown) · runtime config reload
  (`systemctl --user reload`) · reading TigerVNC's viewer and `vncserver` settings
- [x] System keys: the F8 menu sends Ctrl+Alt+Del, Super, Alt+Tab, Alt+F4 and Print Screen (a full keyboard grab
  needs toolkit support; P-49)
- [x] KDE and Xfce certification rows and the macOS checklist in the [compatibility matrix](../testing/compatibility-matrix.md)
- ⚠️ The hand checks in the matrix (KDE, Xfce, COSMIC live, macOS) are still to be done by a person.

## M10: GPU paths and H.264 ⬜ Not started · [council](../council/2026-10-23-initial-roadmap.md#m10-gpu--h264)
SRV-11, ENC-8
- [ ] DMA-BUF capture, VA-API/Vulkan encode, and H.264 behind an **optional** cargo feature
- [ ] **Conditions:** licensing review before enabling in release builds

---

## Horizon (deferred, needs its own council session before scheduling)
- **Web interface over Tailscale** (PRD-9): WebSocket gateway, authenticated, SSRF-safe
- Tight security type (SEC-4)
- Privilege-separated agent (protocol process / capture process)
- Greeter / login-screen access (needs a privileged, audited component)
- Vault export / backup
