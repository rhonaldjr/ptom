# ADR-0003: Viewer UI built on libcosmic (iced)

- **Status:** Accepted · **Date:** 2026-10-23 · **Deciders:** user
- **Requirements:** VWR-1, VWR-12, PRD-3, PRD-5

## Context
COSMIC/Pop!_OS is the top-priority desktop. The viewer needs tabs, multiple windows, a GPU-backed
framebuffer surface and native look on COSMIC, and it must also run on GNOME, KDE and macOS.

## Decision
Build the viewer with **libcosmic** (built on iced, wgpu rendering). The framebuffer is rendered
as a GPU texture updated from damage rectangles.

## Consequences
- Native on COSMIC and acceptable on other DEs. macOS support goes through iced/winit (tier 2).
- libcosmic is distributed via git, not crates.io. Pin it to a commit in `Cargo.lock` and update deliberately.
- The UI crate stays a humble object. Session logic lives in `ptom-client`.

## Alternatives considered
GTK4 (native on GNOME, weaker on macOS, C dependency), egui (fast to build, non-native), and FLTK (upstream TigerVNC, poor Wayland story).
