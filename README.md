# Farsight

**See and control your other computers from wherever you are.**

Farsight is a free, open-source remote desktop app for Linux and macOS. It works with the VNC standard, so it can
connect to the VNC servers you already have, and it is built to replace TigerVNC. It adds the things people
usually miss: finding computers automatically, keeping several desktops open at once, and keeping your
passwords safe.

> **Project status: early development.** The **viewer works**, with **encrypted connections**, **saved
> computers** and **automatic discovery** of computers on your network. It connects to the VNC servers you
> already run (TigerVNC, x11vnc, and others). Farsight's own server is next (M4). See [what's coming](#whats-coming).

---

## What Farsight will do

**The viewer** (`farsight`): the app you use to see other computers
- **Finds computers for you.** It lists the VNC servers on your home network automatically, including
  ones made by other software, and you add them with one click.
- **Many desktops at once**, in tabs or in separate windows.
- **Tray icon** for quick access to your computers, on COSMIC, KDE, Pop!_OS GNOME, Xfce and macOS.
- **Remembers your computers and passwords**, safely encrypted on your device.
- **Shows clearly whether a connection is secure**, and warns you before you use an insecure one.
- Full screen, scaling, clipboard sharing, multiple monitors, and view-only mode.

**The agent** (`farsight-agent`): runs on the computer you want to reach
- Shares your desktop on **Wayland or X11**, with COSMIC (Pop!_OS) first, then GNOME, KDE, Xfce and others.
- Can also run **desktops that keep going in the background**, even when nobody is connected.
- Runs as your normal user, never as an administrator, and lets the person at the screen see when someone is connected.

**Works with what you already use.** Farsight talks to TigerVNC, TightVNC, RealVNC, noVNC, macOS Screen
Sharing and other VNC software.

## What's coming

| Milestone | What you get |
|---|---|
| ✅ M0 | Project foundation: automated testing and releases |
| ✅ M1 | The core VNC protocol, built and tested |
| ✅ M2 | **First usable viewer:** connect to your existing VNC servers |
| ✅ M3 | Secure connections and saved passwords |
| ✅ M3.5 | Automatic discovery of the computers on your network |
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

Download the file for your system, unpack it, and run `farsight`. Distribution packages (Pop!_OS/Ubuntu,
Fedora, Arch, Flatpak) are planned.

## Use the viewer

```bash
farsight                      # opens the "Connect to a computer" screen
farsight pop-os:1             # connects to display 1 on the computer "pop-os"
farsight -ViewOnly pop-os:1   # watch without controlling
```

- Press **F8** for the viewer menu: full screen, view only, Ctrl+Alt+Del, disconnect.
- The window header always shows whether the connection is **encrypted**. Farsight uses encryption whenever the
  server offers it (TigerVNC's X509 and RSA-AES modes). It asks before using an unencrypted connection on your network.
- The first time you connect securely, Farsight shows the computer's **fingerprint** so you can check it, then
  remembers it. If it ever changes, Farsight warns you before anything is sent.
- **Computers on your network** are listed automatically when they announce themselves. To find the rest,
  click **This is my network, scan it** (once per network). Farsight then checks your own network for VNC
  servers, without ever sending a password. Found computers show "identity not verified" until you connect.
- If your desktop has no keyring, Farsight asks for a passphrase to protect your saved computers.
- Computers you connect to are remembered and searchable from the connect screen. Tick **Remember on this computer**
  to save a password; it's stored encrypted, with the key in your system keyring.

## Set up passwords for a Farsight server

`farsight-passwd` replaces `vncpasswd`. It never stores passwords in the weak old format:

```bash
farsight-passwd user add alice      # a user who signs in with a username and password (stored as a secure hash)
farsight-passwd vncauth set         # a password for older viewers (weak; for compatibility only)
farsight-passwd import              # bring over an existing ~/.vnc/passwd
```
- Resizing the window resizes the remote desktop when the server allows it (turn this off with `-RemoteResize=0`).
- Farsight accepts TigerVNC's `vncviewer` options. Run `farsight --help` to see them.

## Build locally

You need [Rust](https://rustup.rs) (the stable toolchain) and, for the viewer, a few system libraries:

```bash
sudo apt install libxkbcommon-dev libwayland-dev libfontconfig-dev libfreetype-dev libdbus-1-dev
```

Or use Docker. `scripts/docker-cargo.sh build -p farsight` builds the viewer without installing anything.

```bash
git clone https://github.com/rhonaldjr/ptom.git
cd ptom
cargo build --release        # programs end up in target/release/
cargo test --workspace       # run the tests
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
- Interop test against a real TigerVNC server:
  ```bash
  docker build -t farsight-interop-tigervnc tests/interop/tigervnc
  docker run -d -p 127.0.0.1:15901:5901 farsight-interop-tigervnc
  FARSIGHT_INTEROP_ADDR=127.0.0.1::15901 cargo test -p farsight-net --test interop_tigervnc
  ```
- Fuzz tests (needs nightly Rust and `cargo install cargo-fuzz`): `cd fuzz && cargo +nightly fuzz run rfb_version`
- Use [Conventional Commits](https://www.conventionalcommits.org) (`feat:`, `fix:`, `docs:` …). Every merge
  to `main` becomes a release, and the commit type decides the version number.
- Design decisions, the roadmap and the reasoning behind them are in [docs/](docs/README.md).

## License

MIT. Farsight is written from scratch from the public VNC specification and contains no code from other VNC projects.
