# ADR-0001: Record architecture decisions

- **Status:** Accepted · **Date:** 2026-10-23 · **Deciders:** user

## Context
Many contributors, both human and AI, need a durable record of why ptom is built the way it is.

## Decision
Use Michael Nygard-style ADRs in `docs/adr/`, numbered sequentially, starting from `0000-template.md`.
Accepted ADRs are immutable. A change of decision is a new ADR that supersedes the old one.

## Consequences
Decisions are discoverable. CLAUDE.md summarises the accepted ones in its "Settled decisions" table.
