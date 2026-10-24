# Roadmap Council

**Rule:** no item enters [ROADMAP.md](../roadmap/ROADMAP.md) without a council session. Debates
are capped at **3 rounds**. If a conflict is still open after round 3, it is **escalated to the
user**, who decides. Every session is recorded in [`docs/council/`](../council/).

## Members
All nine [personas](../personas/README.md). The Business Analyst opens the session and the
Product Owner chairs it. Each persona speaks only from its own grounding framework.

## Procedure

**0. Framing (Business Analyst)**
- Place the proposal on the [story map](../roadmap/story-map.md): activity, task, stories, release slice.
- State the outcome and draft acceptance criteria.

**Round 1: Positions.** Every persona states one of:
- `SUPPORT`, with what it adds.
- `CONCERN`, with a condition that would satisfy it.
- `OBJECT`, with the principle violated and what would resolve it.

Keep it to one or two lines each. If there are no `OBJECT`s and every `CONCERN` is accepted as a
condition, the item is **accepted** and the session skips to the Outcome.

**Round 2: Rebuttal.** Only the personas involved in an objection speak. Proposers may amend
the item (re-slice, add conditions, reorder). The objector answers `WITHDRAW`, `MAINTAIN` or `AMEND`.

**Round 3: Final.** This is the last attempt at a compromise. The Chair proposes a synthesis.
Each remaining objector accepts it or maintains the objection.

**Outcome.** One of:
- `ACCEPTED`: added to the roadmap with its conditions.
- `ACCEPTED-WITH-CONDITIONS`: conditions become acceptance criteria or blocking tasks.
- `DEFERRED`: parked on the Horizon list with the reason.
- `REJECTED`
- `ESCALATED`: still unresolved after round 3. **Stop and ask the user.** Present each side's
  strongest argument, the options, and the council's leaning if there is one. Do not add the item
  until the user answers, then record the user's decision in the session file.

## Tie-breaking guidance (it does not decide outcomes)
- Security and privacy objections about **defaults** win over convenience unless the user
  says otherwise. Making a feature opt-in is usually the compromise.
- Product Owner owns **ordering**. Chief Architect owns **structure**. QA owns **Done**.
- "Drop-in TigerVNC compatibility" is a product promise. Breaking it needs the user's sign-off.

## Session record template
Copy into `docs/council/YYYY-MM-DD-<slug>.md`:

```markdown
# Council: <title>
Date: YYYY-MM-DD · Chair: Product Owner · Items: <ids>

## <Item ID>: <title>
**Framing (BA):** activity › task › story; slice; outcome; acceptance criteria.

**Round 1**
| Persona | Position | Note |
|---|---|---|
| Chief Architect | SUPPORT | … |
| … | … | … |

**Round 2** (only if objections)
**Round 3** (only if still open)
**Outcome:** ACCEPTED / … · Conditions: … · Roadmap: Mx
```
