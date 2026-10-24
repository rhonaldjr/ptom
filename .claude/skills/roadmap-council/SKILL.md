---
name: roadmap-council
description: Run a farsight roadmap council session (9 personas, max 3 debate rounds) on a proposed roadmap item before it is added to docs/roadmap/ROADMAP.md. Use whenever anything is added to or reordered on the roadmap.
---

Follow `docs/process/roadmap-council.md` exactly.

1. **Frame it (Business Analyst).** Place the proposal on `docs/roadmap/story-map.md`, tie it to
   requirement IDs in `docs/spec/requirements.md` (add new IDs if needed), and draft acceptance criteria.
2. **Round 1.** Gather a position from all nine personas. For an independent read, spawn the persona
   subagents in `.claude/agents/` in parallel, passing each one the framing. Otherwise adopt each
   persona in turn after reading its `docs/personas/*.md`.
3. **Rounds 2–3.** Only if there are objections. Proposers may amend. Hard stop after round 3.
4. **Outcome.** ACCEPTED / ACCEPTED-WITH-CONDITIONS / DEFERRED / REJECTED / **ESCALATED**.
   - If ESCALATED: **stop and ask the user** (AskUserQuestion). Present each side's strongest
     argument and the options. Do not add the item until the user answers.
5. **Record it.** Write `docs/council/YYYY-MM-DD-<slug>.md` using the template, then update
   `ROADMAP.md` (with a link to the session anchor), the story map, requirements, and an ADR if a
   structural decision was made. Update `docs/README.md` if you added new docs.
