# Council: System tray icon and discovery of any VNC server

Date: 2026-10-23 · Chair: Product Owner · Items: VWR-15, VWR-16, VWR-17
Round 1 positions were given independently by the nine persona subagents in `.claude/agents/`.
Requested by the user: *"system tray icon for the viewer on all target desktop environments"* and
*"discover all the servers in the network (automatically) and allow adding them to the viewer"*.

Abbreviations: **CA** Chief Architect · **SE** Senior Engineer · **PO** Product Owner · **QA** QA Analyst ·
**SA** Security Architect · **UX** UX Designer · **PA** Privacy Analyst · **EH** Ethical Hacker · **BA** Business Analyst

---

## A: Viewer tray icon
**Framing (BA):** activity 5 "Manage many sessions" and activity 2 "Find a computer". Slice 5. Proposed VWR-15.
The SNI hosts are: COSMIC status-area applet, KDE Plasma, Pop!_OS GNOME (AppIndicator extension), Xfce (SNI plugin) and macOS NSStatusItem. **Vanilla GNOME has no tray host.**

**Round 1**
| Persona | Position | Note |
|---|---|---|
| CA | CONCERN | A `StatusPresenter` port and view model in `ptom-client`, shared with the main window. ksni and macOS adapters in an outer crate. A null adapter when there is no host. ADR for ksni over `tray-icon` (GTK/C). |
| SE | CONCERN | Two backends with different lifecycles. macOS needs Objective-C FFI, so it gets its own crate and ADR (`unsafe` is forbidden in the workspace). "No tray host" is an explicit, tested state, not a swallowed error. Linux SNI comes first. |
| PO | CONCERN | The tray is a shortcut only. Everything in it must also be reachable from the main window (Norman: discoverability). Close-to-tray is **off by default**, with a one-time notice when it is enabled. |
| QA | CONCERN | Write the AC against an explicit DE support matrix. CI tests with a stub SNI watcher on headless D-Bus *and* with no watcher at all. One Q3 charter per DE. The menu model is tested through a fake port. |
| SA | CONCERN | One-click connect from the tray must go through the same fingerprint/TOFU check, must never send saved credentials to an unverified host, and must sanitise names. ksni is the right supply-chain choice. |
| UX | CONCERN | Parity with the main window (#6). The icon reflects state: active sessions, unencrypted (#1). Close-to-tray is off by default or explained (#3). COSMIC applet as the reference (#4). |
| PA | CONCERN | The tray reads only through the vault. It shows labels only, no usernames. While the vault is locked it shows only "Unlock…". There is a setting to hide discovered servers from the tray. Background running is visible to the user. |
| EH | CONCERN | Spoofed names plus one-click connect allow harvesting VncAuth responses (T1557/T1110). Unverified entries are visibly marked and never auto-send credentials. Strip control and bidi characters from names and cap their length (API10). |
| BA | CONCERN | Split into thin stories. First slice: saved sessions plus Quit. Then: raise open session, discovered submenu, stay-running-on-close (its own AC). Explicit DE list and a no-host Given/When/Then. |

No objections. Every concern is accepted as a condition.

**Outcome:** ACCEPTED-WITH-CONDITIONS · Roadmap **M6** · [ADR-0013](../adr/0013-viewer-tray-icon.md)

---

## B: Discover any VNC server (not just ptom agents)
**Framing (BA):** activity 2 "Find a computer". Extends PRD-4. Proposed VWR-16 (discover any RFB server) and VWR-17 (one-click Add).
TigerVNC, x11vnc and QEMU do not advertise over mDNS, so finding "all" servers needs **active probing**:
TCP connect to RFB ports on the local subnet, then read the banner and the security-type list. That is ATT&CK T1046 behaviour.

**Round 1**
| Persona | Position | Note |
|---|---|---|
| CA | CONCERN | The banner/security-list parse is pure protocol and belongs in `ptom-rfb`. mDNS and the probe are two adapters behind the `Discovery` port, feeding one `DiscoveredServer` entity with provenance. Probing is a swappable adapter that the web gateway can omit. New ADR. |
| SE | CONCERN | Probing is a different mechanism from ADR-0010 and needs a new ADR. The probe reuses the existing handshake codec (DRY). Bounded read and a fuzz target. An explicit scope type, no flag args. |
| PO | CONCERN | High value for migrants and homelab admins, whose servers don't advertise. Passive is on by default. Active probing is **opt-in per network** ("This is my network, scan it"), remembered, with visible progress. |
| QA | CONCERN | Fuzz target and proptest for the parser. Injected clock for timeouts. Concrete time bounds instead of "N seconds". Q4 containers: TigerVNC, x11vnc, QEMU, and an mDNS advertiser. A Q2 test proving no bytes are sent after the security list. |
| SA | **OBJECT** | A remote-access tool with a scanner on by default is T1046 + T1219 out of the box (ASVS V14). Condition: passive by default. Active scanning only by user opt-in per network, never on its own on a new network. Local-link/RFC 1918 only, rate-limited. Credentials keyed by verified identity, never by IP. |
| UX | CONCERN | Passive by default. "Scan this network…" explains itself in plain words, is remembered per network, shows progress and can be cancelled. Plain labels. "Add" pre-fills a session and doesn't connect. A helpful empty state. |
| PA | **OBJECT** | Default probing collects data about other people's machines on networks the user doesn't own (PbD #2, #7). Condition: off by default and opt-in per network, with the trusted-network list kept in the vault and clearable. Local subnet only. Results kept in memory until "Add". No banners in logs. A "Scanning…" indicator. |
| EH | **OBJECT** | T1046 from our own binary trips IDS and breaches AUPs (API8). Condition: off by default with per-network consent. Subnet/neighbour table only, capped and rate-limited. Never auth. Fuzzed parser. The "encrypted" label is advisory only (it can be spoofed or downgraded). |
| BA | CONCERN | Split into B1 (passive mDNS for any server, plus Add), B2 (active probing, opt-in, with "no probe without consent" in its AC) and B3 (labels). B1 moves to right after M3. |

**Round 2: amendment (PO and BA)**
B is re-sliced as follows:
- **Passive `_rfb._tcp` browsing for any server is automatic and on by default.**
- **Active probing is automatic only on networks the user has marked as their own.** The user marks a
  network once, through "This is my network, scan it", and the choice is remembered in the vault per
  network and can be cleared. Probing never starts on a network the user hasn't marked.
- Scope is limited to directly attached RFC 1918 / link-local subnets (at most /22 per interface) plus the
  ARP/NDP neighbour table. Ports default to 5900–5910 and are configurable. No more than 32 concurrent
  connects, with a 500 ms connect timeout. The probe reads only the banner and the security-type list,
  then closes. It never sends auth or credentials.
- Results are held in memory and saved through the vault only on "Add". Each entry shows "Identity not
  verified" until TOFU, and an advisory "Not encrypted" label for legacy-only servers.
- Credentials are keyed by verified identity, never by IP.
- Time bounds: advertised servers appear within **5 s**. An opted-in scan of a /24 completes within **30 s**.

The amendment meets each objector's stated Round 1 condition exactly:
- **SA:** WITHDRAW
- **PA:** WITHDRAW
- **EH:** WITHDRAW

All the CONCERN conditions are carried into the AC.

**Ordering (PO, who owns ordering):** B moves to a new slim milestone, **M3.5**, right after M3. It needs
only M3's vault and TOFU, and it proves value with servers people already run. CA, UX, PA, EH, SA and BA
support this. SE preferred M6, worried that the UI would be built before the multi-session model exists.
**PO response:** M3.5 shows a "Computers on your network" list that creates *saved sessions* (which exist
in M3), not tabs, so nothing needs rework. SE accepts. QA's condition holds: the Q4 interop container
matrix lands with M3.5. Agent-side mDNS **advertising** stays in M6.

**Outcome:** ACCEPTED-WITH-CONDITIONS · Roadmap **M3.5** · [ADR-0012](../adr/0012-active-rfb-probing.md)

> **Note for the user.** You asked for discovery that is "automatic". The council kept passive discovery fully
> automatic. Active probing is automatic *after a one-time "this is my network" consent per network*, rather
> than on every network. Tell us if you want something different. Turning probing on by default for every
> network would reopen the three objections.
