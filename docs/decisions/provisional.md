# Provisional decisions

Decisions made during implementation **without the user's input** (the user asked that work continue rather
than wait). Each one is open for review. Once confirmed or overridden, mark it here and, if it is structural,
promote it to an ADR.

| # | Date | Milestone | Decision | Rationale | Status |
|---|---|---|---|---|---|
| P-1 | 2026-10-23 | M2 | Pull ExtendedDesktopSize/SetDesktopSize and Cursor/CursorWithAlpha decoding into M2 ([council](../council/2026-10-23-m2-scope.md)) | VWR-3 (remote resize) needs them, and the viewer is unusable with a laggy remote-drawn pointer | Open |
| P-2 | 2026-10-23 | M2 | Mouse **back** button sends RFB button 8; **forward** is not sent until ExtendedMouseButtons (M9) | The core pointer message has only 8 button bits | Open |
| P-3 | 2026-10-23 | M2 | App ID `ca.akrotechnologies.Farsight` | Reverse-DNS of the maintainer's domain | Open |
| P-4 | 2026-10-23 | M2 | Linux release builds and CI run on **Ubuntu 22.04** (glibc 2.35) | Binaries built on newer glibc don't run on Pop!_OS 22.04 | Open |
| P-5 | 2026-10-23 | M2 | Viewer menu opens with **F8** (TigerVNC default) and offers full screen, view only, Ctrl+Alt+Del and disconnect | Matches TigerVNC muscle memory (Nielsen #4) | Open |
| P-6 | 2026-10-23 | M2 | `deny.toml`: allow the **Unlicense** (libcosmic's `apply`); declare MPL-2.0/MIT for libcosmic crates without a licence field; allow git sources from pop-os, iced-rs and jackpot51; check "unmaintained" advisories for **direct** dependencies only | Needed to depend on libcosmic (ADR-0003). Every licence was checked against the upstream LICENSE files. Transitive unmaintained crates are libcosmic's to fix | Open |
