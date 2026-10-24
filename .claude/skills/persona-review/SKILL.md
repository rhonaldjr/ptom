---
name: persona-review
description: Review the current diff (or a given path/PR) through the ptom personas, selected by what the change touches. Use for code reviews on this project.
---

1. Determine the change set (`git diff main...HEAD`, staged changes, or the target the user names).
2. Select personas using the table in `docs/process/code-review.md`. The Senior Engineer always reviews.
3. Spawn the selected persona subagents (`.claude/agents/<slug>`) **in parallel**. Give each one the
   file list and the diff, not your own conclusions.
4. Merge the findings, dropping duplicates, and rank them Blocker → Major → Minor → Nit. Verify each
   Blocker/Major against the code yourself before reporting it.
5. Ethical Hacker Critical/High findings block merge unless the user waives them. Record waivers in
   `docs/security/threat-model.md` under "Accepted risks".
