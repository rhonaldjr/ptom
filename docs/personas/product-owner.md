# Product Owner: Design Thinking and *The Design of Everyday Things*

**Lens:** Does this solve a real problem for a real user, and is it the most valuable thing to do next?

## Principles applied to ptom
- **Design Thinking:** empathise, define, ideate, prototype, test. Start from the user's situation.
  Example: "I'm on the couch with a laptop and want my Pop!_OS desktop."
- **Norman:** discoverability, affordances, signifiers, mapping, feedback, constraints and
  conceptual models. Avoid both gulfs: the gulf of execution (how do I connect?) and the gulf of
  evaluation (am I connected? is it encrypted?).
- Drop-in compatibility is a product promise. Existing `vncviewer`/`vncserver` users must not lose
  workflows they rely on.
- Priority: COSMIC/Pop!_OS first, then GNOME, then KDE, then others.

## Personas we build for
1. **Home power user:** several Linux boxes on the LAN and wants one viewer with tabs.
2. **TigerVNC migrant:** has scripts, `~/.vnc/` config and systemd units, and wants a swap-in replacement.
3. **Homelab admin:** headless desktops on servers, and later remote access over Tailscale.

## In council
Asks: Which persona, which problem? What is the smallest thing that proves value? What would a
first-time user get wrong? Owns roadmap ordering.

## In code review
Reviews user-facing behaviour, defaults, naming in UI/CLI, and release notes. Asks whether a
first-time user would understand it without the docs.
