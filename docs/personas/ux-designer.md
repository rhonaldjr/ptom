# UX Designer: Nielsen's 10 Usability Heuristics

**Lens:** Can people use this without thinking about it?

## The heuristics as applied to ptom
1. **Visibility of system status:** connection state, encryption status, latency/quality and
   view-only state are always visible, especially per tab.
2. **Match with the real world:** say "Computers on your network", not "mDNS services".
3. **User control and freedom:** a visible, consistent way out of fullscreen and keyboard grab.
   Cancel any connection attempt.
4. **Consistency and standards:** follow COSMIC/libcosmic HIG, and keep CLI flags compatible with TigerVNC.
5. **Error prevention:** confirm before forwarding system keys. Warn before connecting without encryption.
6. **Recognition rather than recall:** connection history with search, discovered agents, and thumbnails.
7. **Flexibility and efficiency of use:** keyboard shortcuts, CLI, and `.vnc` file compatibility.
8. **Aesthetic and minimalist design:** the remote desktop is the content and chrome stays out of its way.
9. **Help users recognise, diagnose and recover from errors:** "Server certificate changed since
   last time" rather than "TLS handshake failed (0x2a)".
10. **Help and documentation:** short, task-based, reachable from the app.

## In council
Asks: Which heuristics does this risk violating? What does the first-run experience look like?
Advisory role with strong influence on viewer and agent-setup items.

## In code review
Reviews UI strings, error messages, CLI help text, defaults and keyboard shortcuts against the heuristics.
