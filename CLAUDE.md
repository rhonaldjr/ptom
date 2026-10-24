# CLAUDE.md — ptom (Peepin Tom)

ptom is an open-source, memory-safe **drop-in replacement for TigerVNC**, written in Rust.
It works on X11 and Wayland and adds features TigerVNC lacks: a multi-session viewer with tabs
and windows, LAN discovery of agents, and an encrypted credential vault.

- `ptom` is the viewer binary. `ptom-agent` is the server/agent. `ptom-passwd` is the password tool.
- Licence: MIT. **Clean-room only.** Never copy, port or paraphrase code from TigerVNC,
  libvncserver, TightVNC or any other GPL VNC implementation. Work from RFC 6143 and the
  community RFB spec (https://github.com/rfbproto/rfbproto). See [ADR-0011](docs/adr/0011-clean-room-licensing.md).

## Settled decisions (do not re-open without a new ADR)

| Topic | Decision | ADR |
|---|---|---|
| Language / architecture | Rust workspace using Clean Architecture layering | [0002](docs/adr/0002-rust-workspace-clean-architecture.md) |
| Viewer UI | libcosmic (iced) | [0003](docs/adr/0003-viewer-ui-libcosmic.md) |
| Platforms | Viewer: Linux (tier 1) and macOS (tier 2). Agent: Linux only. **No Windows.** | [0004](docs/adr/0004-platform-scope.md) |
| Releases | Every push to `main` cuts a semver-tagged GitHub release | [0005](docs/adr/0005-release-per-push.md) |
| Headless desktops | Smithay headless Wayland compositor plus Xwayland | [0006](docs/adr/0006-headless-smithay.md) |
| Crypto / TLS | rustls and RustCrypto. No OpenSSL or GnuTLS. | [0007](docs/adr/0007-crypto-stack.md) |
| Secrets | OS keyring plus an encrypted vault. Never store secrets in plaintext. | [0008](docs/adr/0008-secret-storage.md) |
| Privilege | Agent runs as an unprivileged user service. Never as root. | [0009](docs/adr/0009-least-privilege.md) |
| Discovery | mDNS / DNS-SD `_rfb._tcp` | [0010](docs/adr/0010-lan-discovery.md) |
| Discovery of any VNC server | Passive mDNS always on; active probing only on user-marked networks | [0012](docs/adr/0012-active-rfb-probing.md) |
| Viewer tray icon | `ksni` (SNI) on Linux, NSStatusItem on macOS, behind a port; no-host fallback | [0013](docs/adr/0013-viewer-tray-icon.md) |

Desktop priority order: **COSMIC (Pop!_OS)**, then GNOME (incl. Pop!_OS GNOME), then KDE Plasma, then Xfce and others.

## Repository layout

```
crates/            library crates, inner layers first
  ptom-rfb         ENTITIES: RFB types, wire codec, state machines. Sans-I/O. No async, no OS.
  ptom-codec       ENTITIES: pixel encodings (Raw, CopyRect, RRE, Hextile, ZRLE, Tight, JPEG, H.264)
  ptom-security    ENTITIES/USE CASES: security-type negotiation, VncAuth, RA2, VeNCrypt logic
  ptom-client      USE CASES: viewer session logic, plus ports (traits) the UI and net layers implement
  ptom-server      USE CASES: agent session logic, plus ports (FrameSource, InputSink, Clipboard…)
  ptom-net         ADAPTER: tokio TCP/TLS (rustls) transports, later WebSocket
  ptom-vault       ADAPTER: encrypted credential/session store, OS keyring
  ptom-discovery   ADAPTER: mDNS advertise/browse
apps/              FRAMEWORKS & DRIVERS (composition roots only; thin)
  ptom             viewer (libcosmic)
  ptom-agent       agent daemon
  ptom-passwd      vncpasswd-compatible password tool
docs/              ALL documentation (see docs/README.md)
fuzz/              cargo-fuzz targets for every parser (own workspace, nightly)
.github/workflows  ci.yml (PR gate), release.yml (push to main → release)
```

**Dependency rule:** dependencies point inward only, so apps depend on adapters, which depend on
use cases, which depend on entities. `ptom-rfb` and `ptom-codec` must not depend on tokio,
any I/O crate, or any other workspace crate in an outer layer.
`scripts/check-layers.sh` enforces this in CI.
Platform capture/input adapters (`ptom-capture-wayland`, `ptom-capture-x11`, `ptom-headless`)
are added as crates in their roadmap milestones.

## Commands

```bash
cargo build --workspace
cargo test --workspace --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check                    # licences, advisories, bans (deny.toml)
scripts/check-layers.sh             # Clean Architecture dependency rule
cargo +nightly fuzz run <target>    # from fuzz/ (separate workspace); CI runs each for 30s
```

No local toolchain? `docker run --rm -v "$PWD":/w -w /w rust:latest cargo test --workspace`.

## Engineering rules

- **`unsafe_code = "forbid"`** is set workspace-wide. FFI lives only in a dedicated `*-sys` or adapter
  crate that opts out explicitly, with an ADR and a `// SAFETY:` comment on every block.
- Clean Code: small functions, intention-revealing names, no boolean flag parameters, no
  `unwrap()`/`expect()` outside tests and `main` (clippy enforces this). Errors use `thiserror`
  in libraries and `anyhow` only in `apps/`.
- **Parsers are attack surface.** Every wire decoder must bound allocations using lengths from the
  peer (e.g. ClientCutText, SetEncodings counts, rectangle sizes). It must never panic on
  hostile input, and it needs a fuzz target plus property tests.
- Sans-I/O: protocol state machines take bytes and events in and return bytes and actions out. This keeps
  them unit-testable without sockets and lets the future web gateway reuse them.
- Secrets: hold them in `secrecy::SecretString`/`zeroize` types. Never log them or include them in `Debug`.
- Logging: `tracing`. Do not log framebuffer contents, clipboard contents, keystrokes or credentials,
  not even at trace level.
- Insecure legacy security types (None, VncAuth/DES) exist only for interop. Off non-loopback,
  they must be **opt-in**, and using them must log a warning.

## Testing (Agile Testing Quadrants)

- Q1 (tech-facing, supports the team): unit tests next to the code, plus `proptest` round-trips for every codec.
- Q2 (business-facing, supports the team): acceptance tests in `tests/` per crate, written from story-map stories.
- Q3 (business-facing, critiques the product): exploratory charters and usability sessions, recorded in `docs/testing/`.
- Q4 (tech-facing, critiques the product): fuzzing, `criterion` benchmarks, and an interop matrix
  (TigerVNC, TightVNC, RealVNC, noVNC) in CI containers.
- A bug fix lands together with a test that reproduces the bug.

## Commits and releases

- Use Conventional Commits (`feat:`, `fix:`, `perf:`, `refactor:`, `docs:`, `test:`, `ci:`, `chore:`, `feat!:`).
  The prefix decides the version bump: `feat` bumps minor, everything else bumps patch, `!`/`BREAKING CHANGE` bumps major (minor while 0.x).
- **Commit once per completed roadmap milestone** (M0, M1, …), not per checkbox. Work on the
  milestone branch (`m<N>/<slug>`) and, when every item in the milestone is done, make **one**
  Conventional Commit for the whole milestone. That commit ticks its boxes in
  [ROADMAP.md](docs/roadmap/ROADMAP.md). Squash any interim work commits into it.
- **Update README.md when you complete a milestone**, in the same milestone commit. Write for users:
  plain language, little jargon. Cover what ptom is, what works today, what's coming, how to install,
  and how to use it. Technical detail belongs only in the "Build locally" and "Develop" sections.
  Deeper material goes in `docs/`.
- **Every push to `main` produces a release**, so `main` must always be releasable. Commit on a
  milestone branch (`m<N>/<slug>`). Merge to `main` only through PRs that pass `ci.yml`. Never push
  to `main` without the user's go-ahead.
- The workspace version in `Cargo.toml` stays `0.0.0`. The release workflow stamps the real
  version from the tag. Do not hand-edit it.

## Personas, roadmap council and reviews

The nine personas live in [docs/personas/](docs/personas/README.md). Each one is also a Claude Code
subagent in `.claude/agents/`.

- **Adding to the roadmap:** nothing enters [docs/roadmap/ROADMAP.md](docs/roadmap/ROADMAP.md)
  without a roadmap council session ([process](docs/process/roadmap-council.md), skill `/roadmap-council`).
  Debates are capped at **3 rounds**. If a conflict is still unresolved after round 3, **stop and
  ask the user**. Do not pick a winner. Record each session in `docs/council/`.
- **Code review:** use the persona mapping in [docs/process/code-review.md](docs/process/code-review.md)
  (skill `/persona-review`). The Senior Engineer reviews every change. The Security Architect and
  Ethical Hacker review anything touching parsers, auth, crypto, networking, IPC or privilege.
- **Architecture decisions** become ADRs in `docs/adr/` (copy `0000-template.md`, take the next number).
  Never edit an accepted ADR's decision. Supersede it with a new one.

## Documentation

Everything goes under `docs/`: ADRs, roadmap, council records, personas, process, spec, security
(threat model) and testing. Update `docs/README.md` whenever you add a document.
