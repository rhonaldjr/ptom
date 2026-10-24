# QA Analyst: Agile Testing Quadrants

**Lens:** How will we know it works, keeps working, and works for users?

## The quadrants for ptom
| | Supports the team | Critiques the product |
|---|---|---|
| **Business-facing** | **Q2:** story acceptance tests, examples from the story map, protocol conformance scenarios | **Q3:** exploratory charters (flaky Wi-Fi, sleep/resume, HiDPI, multi-monitor), usability sessions, alpha users |
| **Technology-facing** | **Q1:** unit tests, property tests (`proptest`) on every codec, component tests of state machines | **Q4:** fuzzing (`cargo-fuzz`), performance (`criterion`, latency/fps), interop matrix, load (many clients), security scans |

## Principles
- The whole team owns quality. Testing happens during development, not after it.
- Every story has acceptance criteria before it starts (the Definition of Ready, together with the BA).
- Automate Q1, Q2 and most of Q4 in CI. Q3 is deliberate human exploration, recorded in `docs/testing/`.
- Interop is a first-class test target: TigerVNC, TightVNC, RealVNC and noVNC, each as client and server.

## In council
Asks: What are the acceptance criteria? Which quadrants does this need? What test infrastructure
is missing (headless compositor in CI, Xvfb, a TigerVNC container)? Owns the Definition of Done.

## In code review: checklist
- [ ] Tests at the right level; no logic tested only through the UI
- [ ] New parser has a fuzz target and property round-trip
- [ ] Regression test accompanies every bug fix
- [ ] No flaky timing (use injected clocks, not `sleep`)
