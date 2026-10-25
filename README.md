# Farsight

**See and control your other computers from wherever you are.**

Farsight is a free, open-source remote desktop app for Linux and macOS. It works with the VNC standard, so it can
connect to the VNC servers you already have, and it is built to replace TigerVNC. It adds the things people
usually miss: finding computers automatically, keeping several desktops open at once, and keeping your
passwords safe.

> **Project status: early development.** The **viewer** works, with tabs, a tray icon, encrypted connections,
> saved computers and automatic discovery. The **agent** shares your desktop on Wayland (COSMIC, GNOME, KDE)
> and X11. Persistent background desktops are next (M7). See [what's coming](#whats-coming).

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
| ✅ M4 | Share your COSMIC / Wayland desktop |
| ✅ M5 | Share your X11 desktop |
| ✅ M6 | Tabs, windows and the tray icon |
| ✅ M7 | Desktops that keep running in the background |
| ✅ M8 | Faster, smaller updates: every VNC encoding, automatic JPEG for photos, smooth streaming |
| ✅ M9 | Works like TigerVNC: same command names (optional), physical keys for any layout, lock-key indicators |
| ✅ M10 | **Hardware video (preview):** H.264 encoded on your graphics card (AMD, Intel, NVIDIA). Not in the downloads yet: see below |

The full plan is in [docs/roadmap/ROADMAP.md](docs/roadmap/ROADMAP.md).

## Install

Every update to the project publishes a new release on the
[Releases page](https://github.com/rhonaldjr/ptom/releases), with downloads for:

- Linux (Intel/AMD 64-bit and ARM 64-bit)
- macOS (Apple Silicon). The macOS viewer is supported, but Linux comes first.

**Pop!_OS / Ubuntu (22.04 or newer):** download the `.deb` files and install them:

```bash
sudo apt install ./farsight_*_amd64.deb          # the viewer (connect to other computers)
sudo apt install ./farsight-agent_*_amd64.deb    # the agent (share this computer)
```

Use the `arm64` files on ARM computers. **Other Linux and macOS:** download the `.tar.gz` file for your system,
unpack it, and run `farsight`. On macOS, drag **Farsight.app** from the download into Applications. Fedora, Arch and Flatpak packages are planned.

## Use the viewer

```bash
farsight                      # opens the "Connect to a computer" screen
farsight pop-os:1             # connects to display 1 on the computer "pop-os"
farsight -ViewOnly pop-os:1   # watch without controlling
```

- Find **Farsight** in your app menu, or run `farsight`. Each computer opens in its own **tab**. The **Home** tab lists your computers and the ones on your network.
- Press **F8** for the viewer menu: full screen, view only, Ctrl+Alt+Del, **Move to new window**, disconnect.
- The **tray icon** (COSMIC, KDE, Pop!_OS GNOME, Xfce) gives quick access to open sessions and saved computers.
  Desktops without a tray, such as vanilla GNOME, have everything in the window.
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

## Share this computer (the agent)

After installing `farsight-agent`, open **Farsight Sharing** from your app menu. In it:

1. Set a **password** (you'll type it in the viewer on the other computer).
2. Turn **sharing on**.
3. Under **Who can connect**, choose **Computers on my network**. Network connections are always encrypted.
4. At home or at work, click **This is my network** so viewers there can find this computer by itself.
   It's never announced on networks you haven't marked, like café Wi-Fi.

The window also shows this computer's **addresses** (type one into the viewer) and its **fingerprints**
(the viewer asks you to check one the first time). In the viewer, Farsight computers are listed first
and marked **Farsight agent**.

Prefer the terminal?

```bash
farsight-passwd user add $USER                  # your password for connecting from other computers
farsight-agent --show-identity                  # the fingerprints viewers will ask you to check
farsight-agent --trust-this-network             # let viewers on this network find this computer
systemctl --user enable --now farsight-agent    # start sharing now and at every login
```

- Settings chosen in Farsight Sharing are kept in `~/.config/farsight/agent/settings`
  (`listen = lan`, `announce = yes`, `view-only = no`).
- After changing passwords with `farsight-passwd`, `systemctl --user reload farsight-agent` applies them without
  disconnecting anyone.
- If the viewer says **Connection refused**, sharing is off on that computer or set to "Only this computer".
- On Wayland, your desktop asks once whether Farsight may share the screen. Farsight remembers the answer;
  `farsight-agent --forget-portal-approval` forgets it.
- When **you** connect with your own account, you get straight in. Anyone else has to be approved by whoever is at the
  screen (it's denied after 30 seconds). A tray icon and a notification always show when someone is viewing,
  with **Disconnect everyone**.

## Coming from TigerVNC

- Install the optional `farsight-tigervnc-compat` package and `vncviewer`, `vncserver`, `x0vncserver`, `w0vncserver`
  and `vncpasswd` run Farsight, with TigerVNC's options (it replaces the TigerVNC packages rather than hiding them).
- `vncserver :1 -geometry 1280x800` starts a headless desktop in the background; `vncserver -list` and
  `vncserver -kill :1` work too.
- The viewer picks up your TigerVNC viewer settings (quality, view-only and so on) as its defaults, and `vncserver`
  reads `geometry` and `localhost` from TigerVNC's config. Farsight never changes those files.
- Bring your old VNC password over once with `farsight-passwd import`.

## Desktops that keep running (headless)

Like TigerVNC's `vncserver`, the agent can start a **new desktop** that has no screen and keeps running when nobody is
connected. Come back later and everything is where you left it.

```bash
farsight-passwd user add $USER                              # once, if you haven't already
systemctl --user enable --now farsight-headless@5901         # a desktop on port 5901 (display :1)
systemctl --user enable --now farsight-headless@5901.socket  # or: start it on the first connection
```

- Connect to it with `farsight localhost:1` (or `yourcomputer:1` once you allow your network with `--listen lan`).
- Your apps come from `~/.config/farsight/headless-startup`. The first time, it opens a terminal and a file manager;
  edit it to start what you like.
- Resize the viewer window and the desktop follows. Several monitors work too.
- Several people can watch at once, but **only one controls the mouse and keyboard**. Control passes on when that
  person leaves or stops for 10 seconds, and you (the owner) can always take over. A red frame inside the desktop
  shows when more than one person is connected.
- Run one by hand with `farsight-agent --backend headless --display :1 --geometry 1920x1080`.
- Wayland apps work everywhere; X11 apps need Xwayland 23.1 or newer (Ubuntu and Pop!_OS 24.04 and later).

## Set up passwords for a Farsight server

`farsight-passwd` replaces `vncpasswd`. It never stores passwords in the weak old format:

```bash
farsight-passwd user add alice      # a user who signs in with a username and password (stored as a secure hash)
farsight-passwd vncauth set         # a password for older viewers (weak; for compatibility only)
farsight-passwd import              # bring over an existing ~/.vnc/passwd
```
- Resizing the window resizes the remote desktop when the server allows it (turn this off with `-RemoteResize=0`).
- Keys are sent by their position on the keyboard when the server supports it, so any keyboard layout works.
- The F8 menu sends keys your own desktop would otherwise catch: Ctrl+Alt+Del, Super, Alt+Tab, Alt+F4, Print Screen.
- Photos and videos are sent as JPEG to save bandwidth; text and windows stay sharp. Use `-NoJPEG` for exact
  pixels everywhere, or `-QualityLevel 0-9` to trade quality for speed.
- Farsight accepts TigerVNC's `vncviewer` options. Run `farsight --help` to see them.

## Smoother video with H.264 (preview)

Farsight can send the screen as H.264 video, encoded on your graphics card: AMD and Intel through VA-API, NVIDIA
through NVENC. Where there's no suitable card, it uses Cisco's free OpenH264 library instead. That makes scrolling,
video and fast-changing screens much smoother on slow networks. It's off by default, and the downloads don't
include it yet. It goes into the downloads once it has been tested by hand on real AMD and NVIDIA machines
([test plan](docs/testing/charters/2026-10-25-gpu-h264.md)).

To try it, build it yourself (see below) with the H.264 features, then:

```bash
farsight-agent --install-openh264    # once: Cisco's library (needed to view, optional to share)
farsight-agent --check-h264          # which graphics card will encode, and how fast
farsight-agent --h264 --listen lan   # share with H.264
farsight --install-openh264          # once, on the viewing computer
farsight -H264 192.168.1.165         # view with H.264
```

Both sides fall back to the normal, lossless updates whenever H.264 isn't available.

## Build locally

You need [Rust](https://rustup.rs) (the stable toolchain) and, for the viewer, a few system libraries:

```bash
sudo apt install libxkbcommon-dev libwayland-dev libfontconfig-dev libfreetype-dev libdbus-1-dev libpixman-1-dev
```

Or use Docker. `scripts/docker-cargo.sh build -p farsight` builds the viewer without installing anything.

The agent's Wayland support needs PipeWire's development files and is left out of plain `cargo build`. Build it in
Docker with `scripts/docker-cargo.sh build -p farsight-agent --features wayland`.

```bash
git clone https://github.com/rhonaldjr/ptom.git
cd ptom
cargo build --release        # programs end up in target/release/
cargo test --workspace       # run the tests
```

H.264 (preview) is behind optional features. Graphics drivers are loaded when the program runs, so building needs
nothing extra:

```bash
cargo build --release -p farsight-agent -p farsight-passwd \
  --features farsight-agent/h264,farsight-agent/vaapi,farsight-agent/nvenc
cargo build --release -p farsight --features farsight/h264
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
- Benchmarks: `cargo bench -p farsight-codec -p farsight-server` (results and baselines in
  [docs/performance/baselines.md](docs/performance/baselines.md)).
- Headless desktop tests (need Xwayland 23.1+ for the X11 part, and foot): `cargo test -p farsight-headless` and
  `tests/e2e/agent-headless.sh`.
- Interop test against a real TigerVNC server:
  ```bash
  docker build -t farsight-interop-tigervnc tests/interop/tigervnc
  docker run -d -p 127.0.0.1:15901:5901 farsight-interop-tigervnc
  FARSIGHT_INTEROP_ADDR=127.0.0.1::15901 cargo test -p farsight-net --test interop_tigervnc
  ```
- GPU H.264 check on a computer with a graphics card (CI has none): `tests/e2e/agent-h264-gpu.sh` and
  `cargo test -p farsight-hwenc --features vaapi`. All `unsafe` code lives in `crates/farsight-gpu-sys`
  ([ADR-0021](docs/adr/0021-unsafe-confined-to-gpu-sys.md)); its native declarations are regenerated with
  `scripts/gen-va-bindings.sh` and `scripts/gen-nvenc-bindings.sh`.
- Fuzz tests (needs nightly Rust and `cargo install cargo-fuzz`): `cd fuzz && cargo +nightly fuzz run rfb_version`
- Use [Conventional Commits](https://www.conventionalcommits.org) (`feat:`, `fix:`, `docs:` …). Every merge
  to `main` becomes a release, and the commit type decides the version number.
- Design decisions, the roadmap and the reasoning behind them are in [docs/](docs/README.md).

## License

MIT. Farsight is written from scratch from the public VNC specification and contains no code from other VNC projects.

This software is based in part on the work of the Independent JPEG Group (through the `jpeg-encoder` crate, used
for Tight JPEG).

OpenH264 Video Codec provided by Cisco Systems, Inc. Farsight doesn't include the codec. When you ask for H.264, it
downloads Cisco's own build and checks it ([licensing review](docs/security/h264-licensing.md)).
