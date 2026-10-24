# User Story Map

Owner: Business Analyst. Format: Jeff Patton's story map. The **backbone** is user activities, left
to right in narrative order. Under each are its **tasks**, and the **release slices** run top to bottom.
Requirement IDs refer to [requirements.md](../spec/requirements.md).

## Backbone

| 1. Install & set up | 2. Find a computer | 3. Connect securely | 4. Use the remote desktop | 5. Manage many sessions | 6. Host / share my desktop | 7. Operate & maintain |
|---|---|---|---|---|---|---|
| Install viewer · Install agent · Set password · Migrate from TigerVNC | Type host · Browse LAN · Search history | Negotiate security · Verify identity · Enter/recall credentials | See screen · Type & click · Clipboard · Resize/scale · Fullscreen · System keys | Tabs · Windows · Saved sessions · Status per session | Share live desktop · Run headless desktop · Approve/deny · See who's connected | Upgrade · Configure · Logs · Systemd |

## Release slices

### Slice 1: Walking skeleton (M0–M1)
- As a contributor, I can build, test and release farsight from CI. *(PRD-6, PRD-7)*
- As a developer, I can run a farsight-rfb handshake against a recorded TigerVNC transcript in a test. *(NET-1, NET-6)*

### Slice 2: "View my existing VNC server" (M2–M3)
- As a **TigerVNC migrant**, I want to open `farsight host:1` and see and control my desktop, so that I can switch viewers today. *(VWR-1, NET-*, INP-1/2)*
- As a **home power user**, I want to see that a connection is unencrypted, so that I can make an informed choice. *(VWR-14)*
- As a **home power user**, I want the viewer to remember my server and password securely, so that I can reconnect in one click. *(VWR-8, VWR-9, PRD-10)*
- As anyone, I want the viewer to verify the server's identity, so that nobody can intercept my session. *(SEC-5, SEC-6, SEC-9)*

### Slice 2b: "Find the computers I already have" (M3.5)
- As a **TigerVNC migrant**, I want the viewer to find my existing VNC servers automatically, even ones that don't advertise, so that I don't need to remember IPs and display numbers. *(VWR-16)*
- As a **home power user**, I want to mark my home network as mine once, so that full scanning happens there and nowhere else. *(VWR-16)*
- As a **home power user**, I want to add a found server in one click, so that it becomes a saved session. *(VWR-17)*

### Slice 3: "Reach my Pop!_OS COSMIC desktop from the couch" (M4)
- As a **home power user**, I want to share my live COSMIC session with a farsight agent running as me, so that I can use it from another room. *(SRV-3, SRV-4, PRD-8)*
- As the **person at the screen**, I want to see when someone is connected and stop them, so that I'm never watched unknowingly. *(SRV-12)*

### Slice 4: X11 parity (M5)
- As a **TigerVNC migrant** on Pop!_OS GNOME (X11), I want `x0vncserver`-style sharing, so that I don't lose my setup. *(SRV-2)*

### Slice 5: "All my machines in one place" (M6)
- As a **home power user**, I want to reconnect to a saved machine from the panel, so that I don't have to open the viewer first. *(VWR-15)*
- As a **home power user**, I want the tray to show my open sessions and newly found computers, so that I can jump straight to them. *(VWR-15)*
- As a **home power user**, I want sessions in tabs or windows, so that I can work across machines. *(PRD-3, VWR-12)*

### Slice 6: "Desktops that keep running" (M7)
- As a **homelab admin**, I want persistent headless desktops, so that my session survives disconnects. *(SRV-1, SRV-5)*

### Slice 7+: Fast and compatible (M8–M10)
Performance, the full encoding set, interop certification, GPU/H.264.

### Horizon
Web access over Tailscale (PRD-9), and greeter/login-screen access.
