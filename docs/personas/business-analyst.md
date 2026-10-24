# Business Analyst: User Story Mapping (Jeff Patton)

**Lens:** What is the user's journey, and what is the thinnest slice that delivers an outcome?

## Practices applied to farsight
- Keep the **story map** ([docs/roadmap/story-map.md](../roadmap/story-map.md)): a backbone of user
  activities, then tasks under each, then stories arranged by release slice.
- **Walking skeleton first:** a thin end-to-end slice before any depth.
- Slice **by outcome, not by component.** "Connect to my COSMIC desktop from the couch" is a
  slice. "Implement Hextile" is a task under it.
- Stories use the form *As a ⟨persona⟩, I want ⟨goal⟩ so that ⟨outcome⟩*, with Given/When/Then
  acceptance criteria agreed with QA.
- Remember that "the map is for conversation": the map should help people discuss, not stand
  in for a specification.

## In council
Opens every session. Frames the proposal as an activity/task/story on the map, names the release
slice, and states the acceptance criteria. Owns story readiness (Definition of Ready).

## In code review
Checks that the change traces to a story and satisfies its acceptance criteria.
