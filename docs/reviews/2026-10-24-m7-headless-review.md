# Persona review: M7 headless desktops (2026-10-24)

Reviewers: Senior Engineer, Security Architect, Ethical Hacker (subagents, read-only). Scope: `farsight-headless`,
`farsight-server::Control`, the agent's headless wiring, systemd units, packaging.

| # | Finding (who) | Severity | Resolution |
|---|---|---|---|
| 1 | Headless + loopback + no passwords offered security type None with no prompt: any local user (or SSH port-forward) gets a shell as the owner (SA, EH) | **Blocker** | **Fixed:** headless refuses to start without a password; None never offered for headless |
| 2 | A watching viewer could resize the shared desktop (bypassing U-9), up to 8192² (SA, EH) | Major | **Fixed:** resize behind the control gate; ≤ 33.5 MP; one resize per 250 ms |
| 3 | Xwayland socket world-connectable between bind and chmod with a permissive umask (SA, EH) | Major | **Fixed:** agent umask 077 before spawning (session gets the user's umask back); chmod right after spawn, never through symlinks |
| 4 | Desktop clipboard went to every viewer, including passive watchers (EH) | Major | **Fixed:** only the controller or the owner receives it |
| 5 | Held keys/buttons stuck after hand-over or controller disconnect (SE, SA, EH) | Major | **Fixed:** `release_all` on take-over and controller disconnect; Shift fixed both ways (unit test) |
| 6 | A stale old-size frame could undo a resize and drop monitors (SE) | Major | **Fixed:** frames carry a layout generation; older ones are dropped |
| 7 | Compositor never stopped; session processes orphaned; blocking `wait` (SE) | Major | **Fixed:** SIGTERM handled, compositor stopped and joined; whole process group TERM then KILL after 2 s |
| 8 | Full framebuffer cloned every tick while the agent was behind (SE) | Major | **Fixed:** no render/copy without channel capacity; unsent damage capped at 64 rects |
| 9 | Focus went to the closing window; X11 unmap didn't refocus (SE) | Major | **Fixed:** focus the topmost live, non-override-redirect window |
| 10 | Untrusted `/tmp/.X11-unix` accepted (SA, EH) | Minor | **Fixed:** must be root- or user-owned and sticky, else no Xwayland |
| 11 | Unbounded clipboard threads; `thread::spawn` panic (SA, EH) | Minor | **Fixed:** at most 8 transfers, spawn errors handled |
| 12 | Startup script ownership not checked (SA) | Minor | **Fixed:** must be the user's and not group/world-writable |
| 13 | Only the first systemd socket used (SA, EH) | Minor | **Fixed:** more than one passed socket is refused; unit comments corrected |
| 14 | Cursor change lost when the channel is full; failed reconfigure half-applied (SE) | Minor | **Fixed** |
| 15 | Boolean flag parameter in `Control::request` (SE) | Minor | **Fixed:** `Role::{Owner, Guest}` |
| 16 | Unit hardening could do more (SA) | Nit | **Fixed:** LockPersonality, RestrictRealtime, RestrictAddressFamilies; password note |
| 17 | A controller can keep control indefinitely by sending input (EH) | By design | **Accepted risk** (P-38), recorded in the threat model; the owner can always take over |
