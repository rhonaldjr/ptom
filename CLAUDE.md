# CLAUDE.md — Farsight

Farsight is an open-source, memory-safe **drop-in replacement for TigerVNC**, written in Rust.
It works on X11 and Wayland and adds features TigerVNC lacks: a multi-session viewer with tabs
and windows, LAN discovery of agents, and an encrypted credential vault.

- `farsight` is the viewer binary. `farsight-agent` is the server/agent. `farsight-passwd` is the password tool.
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
| Protocol core | Sans-I/O client/server state machines; pluggable `Authenticator`; transport upgrades done by the caller | [0015](docs/adr/0015-sans-io-connections.md) |
| RSA-AES crypto | aws-lc-rs for RSA (Marvin-safe), RustCrypto EAX/AES | [0016](docs/adr/0016-ra2-crypto-aws-lc.md) |
| Server identity | Fingerprint pinning (TOFU), checked before credentials; changed identity needs explicit acknowledgement; downgrade refused | [0017](docs/adr/0017-server-identity-verification.md) |
| Agent | Server core + capture backends + desktop integration; accept prompt for non-owner logins; loopback default | [0018](docs/adr/0018-agent-architecture.md) |
| Discovery | mDNS / DNS-SD `_rfb._tcp` | [0010](docs/adr/0010-lan-discovery.md) |
| Agent credentials | Argon2id user hashes; legacy VncAuth sealed in the keyring-backed vault; never write `~/.vnc/passwd` | [0014](docs/adr/0014-agent-credentials.md) |
| Unsafe code | Only in `farsight-gpu-sys`, one op per block with `// SAFETY:`; native GPU libraries loaded at run time; bindings generated and committed | [0021](docs/adr/0021-unsafe-confined-to-gpu-sys.md) |
| Headless desktop details | Smithay 0.7, pixman, Xwayland 23.1+; one controller at a time; `farsight-headless@<port>` units | [0020](docs/adr/0020-headless-desktops.md) |
| Agent advertising | Only on networks the user marked as theirs (`--trust-this-network`) | [0019](docs/adr/0019-advertise-on-trusted-networks.md) |
| Discovery of any VNC server | Passive mDNS always on; active probing only on user-marked networks | [0012](docs/adr/0012-active-rfb-probing.md) |
| Viewer tray icon | `ksni` (SNI) on Linux, NSStatusItem on macOS, behind a port; no-host fallback | [0013](docs/adr/0013-viewer-tray-icon.md) |

Desktop priority order: **COSMIC (Pop!_OS)**, then GNOME (incl. Pop!_OS GNOME), then KDE Plasma, then Xfce and others.

## Repository layout

```
crates/            library crates, inner layers first
  farsight-rfb         ENTITIES: RFB types, wire codec, state machines. Sans-I/O. No async, no OS.
  farsight-codec       ENTITIES: pixel encodings (Raw, CopyRect, RRE, Hextile, ZRLE, Tight, JPEG, H.264)
  farsight-security    ENTITIES/USE CASES: security-type negotiation, VncAuth, RA2, VeNCrypt logic
  farsight-client      USE CASES: viewer session logic, plus ports (traits) the UI and net layers implement
  farsight-server      USE CASES: agent session logic, plus ports (FrameSource, InputSink, Clipboard…)
  farsight-net         ADAPTER: tokio TCP/TLS (rustls) transports, later WebSocket
  farsight-vault       ADAPTER: encrypted credential/session store, OS keyring
  farsight-discovery   ADAPTER: mDNS advertise/browse, opt-in RFB prober, network identity
  farsight-desktop     ADAPTER: notifications, approval prompt, tray icons (agent presence, viewer)
  farsight-capture-x11 ADAPTER: X11 capture/input (x11rb: XDamage, MIT-SHM, XTest, XFixes)
  farsight-capture-wayland ADAPTER: portal + PipeWire capture/input (`wayland` feature; P-24)
  farsight-headless    ADAPTER: headless Smithay compositor + Xwayland for persistent desktops (M7, ADR-0020)
  farsight-h264        ADAPTER: Cisco's OpenH264, downloaded on request and SHA-256 checked (M10, U-12)
  farsight-hwenc       ADAPTER: GPU H.264 (VA-API, NVENC): header writer, colour conversion, encoder threads (M10)
  farsight-gpu-sys     ADAPTER: the ONLY crate with `unsafe`: DMA-BUF reads, libva and NVENC FFI (ADR-0021)
apps/              FRAMEWORKS & DRIVERS (composition roots only; thin)
  farsight             viewer (libcosmic)
  farsight-agent       agent daemon
  farsight-passwd      vncpasswd-compatible password tool
  farsight-sharing     "Farsight Sharing" window for the agent (libcosmic)
docs/              ALL documentation (see docs/README.md)
fuzz/              cargo-fuzz targets for every parser (own workspace, nightly)
.github/workflows  ci.yml (PR gate), release.yml (push to main → release)
```

**Dependency rule:** dependencies point inward only, so apps depend on adapters, which depend on
use cases, which depend on entities. `farsight-rfb` and `farsight-codec` must not depend on tokio,
any I/O crate, or any other workspace crate in an outer layer.
`scripts/check-layers.sh` enforces this in CI.

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
- **Keep ROADMAP.md progress visible** (user instruction). Items are GitHub checkboxes (`- [x]` done, `- [ ]` not);
  each milestone heading says ✅ Done or ⬜ Not started, and the "Status at a glance" table at the top must match.
  Tick items and update the table in the same commit that finishes them.
- **Update README.md when you complete a milestone**, in the same milestone commit. Write for users:
  plain language, little jargon. Cover what Farsight is, what works today, what's coming, how to install,
  and how to use it. Technical detail belongs only in the "Build locally" and "Develop" sections.
  Deeper material goes in `docs/`.
- **Every push to `main` produces a release**, so `main` must always be releasable. Commit on a
  milestone branch (`m<N>/<slug>`). Merge to `main` only through PRs that pass `ci.yml`.
- **Push each finished milestone branch to GitHub** (`git push -u origin m<N>/<slug>`) right after its
  milestone commit (user instruction). Never push WIP commits.
- **Merge each finished milestone to `main`** (user decision U-8): once the milestone passes fmt, clippy,
  tests, deny and the layer check locally, fast-forward `main` to it and push. That publishes a release with
  `.deb` files. Watch the release run and fix CI failures straight away. Anything that isn't a finished,
  checked milestone still needs the user's go-ahead before it goes to `main`.
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

## Provisional decisions
When a question comes up mid-build and the user hasn't answered within about 5 minutes (or isn't
available), make the most conservative reasonable decision, log it in
[docs/decisions/provisional.md](docs/decisions/provisional.md), and keep going. Never block a build on a question.

## Documentation

Everything goes under `docs/`: ADRs, roadmap, council records, personas, process, spec, security
(threat model) and testing. Update `docs/README.md` whenever you add a document.
