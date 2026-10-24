# ADR-0005: Versioned release on every push to main

- **Status:** Accepted · **Date:** 2026-10-23 · **Deciders:** user
- **Requirements:** PRD-7

## Decision
- `.github/workflows/release.yml` runs on every push to `main`. It reruns the full CI gate,
  computes the next semver with **git-cliff** from Conventional Commits, stamps the workspace
  version, builds artifacts (Linux x86_64/aarch64, macOS aarch64), attaches checksums and
  **build provenance attestations**, tags `vX.Y.Z`, and publishes a GitHub Release with generated notes.
- The `Cargo.toml` workspace version stays `0.0.0` in git. The git tag is the source of truth.
- Bump rules while the version is 0.x: `feat` → minor, other types → patch, breaking → minor.
  From 1.0 onward, breaking → major.

## Consequences
- **`main` must always be releasable.** Branch protection requires a PR, a green `ci.yml` and a review.
- A high version count is expected and accepted.
- Releases are immutable. A bad release is fixed with a new push, never by retagging.

## Alternatives considered
Nightly plus release-please stable releases (rejected by the user as too indirect).
