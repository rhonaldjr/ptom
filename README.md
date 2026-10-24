# ptom (Peepin Tom)

**See and control your other computers from wherever you are.**

ptom is a free, open-source remote desktop app for Linux and macOS. It works with the VNC standard, so it can
connect to the VNC servers you already have, and it is built to replace TigerVNC. It adds the things people
usually miss: finding computers automatically, keeping several desktops open at once, and keeping your
passwords safe.

> **Project status: early development.** The foundation is in place, but ptom can't connect to anything
> yet. The first usable version will be the viewer (milestone M2). See [what's coming](#whats-coming).

---

## What ptom will do

**The viewer** (`ptom`): the app you use to see other computers
- **Finds computers for you.** It lists the VNC servers on your home network automatically, including
  ones made by other software, and you add them with one click.
- **Many desktops at once**, in tabs or in separate windows.
- **Tray icon** for quick access to your computers, on COSMIC, KDE, Pop!_OS GNOME, Xfce and macOS.
- **Remembers your computers and passwords**, safely encrypted on your device.
- **Shows clearly whether a connection is secure**, and warns you before you use an insecure one.
- Full screen, scaling, clipboard sharing, multiple monitors, and view-only mode.

**The agent** (`ptom-agent`): runs on the computer you want to reach
- Shares your desktop on **Wayland or X11**, with COSMIC (Pop!_OS) first, then GNOME, KDE, Xfce and others.
- Can also run **desktops that keep going in the background**, even when nobody is connected.
- Runs as your normal user, never as an administrator, and lets the person at the screen see when someone is connected.

**Works with what you already use.** ptom talks to TigerVNC, TightVNC, RealVNC, noVNC, macOS Screen
Sharing and other VNC software.

## What's coming

| Milestone | What you get |
|---|---|
| ✅ M0 | Project foundation: automated testing and releases |
| M1 | The core VNC language, built and tested |
| M2 | **First usable viewer:** connect to your existing VNC servers |
| M3 | Secure connections and saved passwords |
| M3.5 | Automatic discovery of the computers on your network |
| M4 | Share your COSMIC / Wayland desktop |
| M5 | Share your X11 desktop |
| M6 | Tabs, windows and the tray icon |
| M7 | Desktops that keep running in the background |
| M8–M10 | Speed, wider compatibility, and hardware acceleration |

The full plan is in [docs/roadmap/ROADMAP.md](docs/roadmap/ROADMAP.md).

## Install

Every update to the project publishes a new release on the
[Releases page](https://github.com/rhonaldjr/ptom/releases), with downloads for:

- Linux (Intel/AMD 64-bit and ARM 64-bit)
- macOS (Apple Silicon). The macOS viewer is supported, but Linux comes first.

Download the file for your system, unpack it, and run `ptom`. Distribution packages (Pop!_OS/Ubuntu,
Fedora, Arch, Flatpak) are planned.

> Until the first usable milestone, released programs only print their version.

## Build locally

You need [Rust](https://rustup.rs) (the stable toolchain), or Docker.

```bash
git clone https://github.com/rhonaldjr/ptom.git
cd ptom
cargo build --release        # programs end up in target/release/
cargo test --workspace       # run the tests
```

Without installing Rust:

```bash
docker run --rm -v "$PWD":/w -w /w rust:latest cargo test --workspace
```

## Develop

- Read [CLAUDE.md](CLAUDE.md) first. It covers project rules, layout, coding standards and how changes are reviewed.
- Before opening a pull request, run:
  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  cargo test --workspace
  scripts/check-layers.sh       # needs jq
  ```
- Fuzz tests (needs nightly Rust and `cargo install cargo-fuzz`): `cd fuzz && cargo +nightly fuzz run rfb_version`
- Use [Conventional Commits](https://www.conventionalcommits.org) (`feat:`, `fix:`, `docs:` …). Every merge
  to `main` becomes a release, and the commit type decides the version number.
- Design decisions, the roadmap and the reasoning behind them are in [docs/](docs/README.md).

## License

MIT. ptom is written from scratch from the public VNC specification and contains no code from other VNC projects.
