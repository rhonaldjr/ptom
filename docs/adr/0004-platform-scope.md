# ADR-0004: Platform scope: Linux and macOS viewer, Linux agent, no Windows

- **Status:** Accepted · **Date:** 2026-10-23 · **Deciders:** user
- **Requirements:** PLT-1, PLT-2, VWR-1, SRV-10

## Context
The brief contradicted itself (a "linux & macOS" viewer vs a "Linux, Windows" viewer).

## Decision
- **Viewer:** Linux is tier 1 (release-blocking). macOS is tier 2: built and tested in CI, and
  released, but failures there do not block a release.
- **Agent:** Linux only.
- **Windows:** out of scope. This includes the Windows viewer and Windows screen-change hooking.

## Consequences
Less platform code. Windows users can use the viewer through other means or a future web client.
Bringing Windows back requires a new ADR and a council session.
