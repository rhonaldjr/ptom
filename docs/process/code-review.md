# Persona Code Review

Run with the `/persona-review` skill, or spawn the persona subagents in `.claude/agents/`.

## Who reviews what
| Change touches | Required personas |
|---|---|
| Anything | **Senior Engineer** |
| New crate, crate dependency, public trait/port, cross-layer move | **Chief Architect** |
| Wire parsers, encoders/decoders, auth, crypto, TLS, networking, IPC, systemd units, privileges | **Security Architect** and **Ethical Hacker** |
| Storage, logging, discovery/mDNS, clipboard, telemetry, session indicators | **Privacy Analyst** |
| Viewer UI, CLI flags/help, error messages, defaults | **UX Designer**, then **Product Owner** |
| Tests, CI, test infrastructure; any feature (DoD check) | **QA Analyst** |
| Feature PR closing a story | **Business Analyst** (traceability to acceptance criteria) |

## Severity
- **Blocker:** a security or privacy defect, data loss, a crash on hostile input, or a broken
  dependency rule. Must be fixed before merge.
- **Major:** a correctness bug, a missing test for new behaviour, or a UX heuristic violation on a
  primary flow.
- **Minor / Nit:** naming, structure or style beyond what fmt/clippy catch.

A Critical/High finding from the Ethical Hacker can only be waived by the user, and the waiver is
recorded in `docs/security/threat-model.md` under "Accepted risks".

## Output format
Each persona reports `file:line`, severity, the principle cited, the issue, and a suggested fix. If a
persona finds nothing, it says so in one line. Persona review does not replace `cargo clippy`/`cargo test`.
