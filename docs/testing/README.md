# Testing

Strategy: the Agile Testing Quadrants (see the [QA Analyst](../personas/qa-analyst.md) persona and CLAUDE.md).

- **Q1/Q2** live in the code (`#[cfg(test)]` modules, `crates/*/tests/`).
- **Q4** lives in `fuzz/`, `benches/` and the interop jobs in CI.
- **Q3** exploratory charters and their session notes go in this folder as `charters/YYYY-MM-DD-<slug>.md`.

Charter template: *Explore ⟨target⟩ with ⟨resources⟩ to discover ⟨information⟩.* Record setup
(distro, DE, Wayland/X11, network), what you did, what you found, and any bugs filed.
