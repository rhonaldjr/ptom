# Personas

These nine personas sit on the **roadmap council** ([process](../process/roadmap-council.md)) and do
**persona code reviews** ([mapping](../process/code-review.md)). Each one is also a Claude Code
subagent under `.claude/agents/`, so it can be spawned for an independent read.

| Persona | Grounding | Primary concern | Holds veto on |
|---|---|---|---|
| [Chief Architect](chief-architect.md) | Clean Architecture (Robert C. Martin) | Boundaries, dependency rule, longevity | Layering violations |
| [Senior Engineer](senior-engineer.md) | Clean Code (Robert C. Martin) | Readability, correctness, maintainability | Untestable / unreadable code |
| [Product Owner](product-owner.md) | Design Thinking; *The Design of Everyday Things* (Don Norman) | User value, priority, scope | Roadmap ordering |
| [QA Analyst](qa-analyst.md) | Agile Testing Quadrants (Marick; Crispin & Gregory) | Testability, quality evidence | Definition of Done |
| [Security Architect](security-architect.md) | OWASP (ASVS, Top 10), MITRE ATT&CK | Threat model, secure defaults | Insecure defaults, crypto choices |
| [UX Designer](ux-designer.md) | Nielsen's 10 usability heuristics | Usability of viewer, agent setup, CLI | — (advisory) |
| [Privacy Analyst](privacy-analyst.md) | Privacy by Design (Ann Cavoukian), 7 principles | Data minimisation, privacy defaults | Data collection / retention |
| [Ethical Hacker](ethical-hacker.md) | OWASP API Security Top 10; MITRE ATT&CK; business-logic abuse | Concrete attacks on what we build | — (raises blocking findings) |
| [Business Analyst](business-analyst.md) | User Story Mapping (Jeff Patton) | Story map, slicing, acceptance criteria | Story readiness |

A "veto" does not decide anything by itself. It forces another debate round, and if the issue is
still open after round 3, the user decides.

> Note on the original brief: "MITRE ATT&CK techniques, and business-logic abuse analysis" was a
> separate bullet but continues the Ethical Hacker's description. There are nine personas, not ten.
