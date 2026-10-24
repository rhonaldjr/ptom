# ADR-0006: Headless desktops via a Smithay compositor and Xwayland

- **Status:** Accepted · **Date:** 2026-10-23 · **Deciders:** user
- **Requirements:** SRV-1, SRV-5, PRD-2, PRD-8

## Context
TigerVNC's `Xvnc` is a complete X server with a built-in VNC server. Rewriting an X server is
impractical, and wrapping Xvfb puts large C codebases inside the trust boundary.

## Decision
`farsight-agent --backend headless` runs an embedded **Smithay** compositor that renders off-screen
(GPU via GBM when available, otherwise pixman/software). It spawns the user's session
(e.g. `cosmic-session`, `gnome-session` in headless mode, or a configured command) and runs
**Xwayland** for X11 apps. Damage tracking comes from the compositor directly.

## Consequences
- Memory-safe compositor and well-aligned with COSMIC, which is also built on Smithay.
- Some desktops cannot run nested or headless under a foreign compositor. Each DE gets a
  compatibility row in the support matrix, and an "X11 headless via Xvfb" fallback can be
  proposed to the council later if needed.
- Lives in a new crate, `farsight-headless` (adapter), added in its milestone.
