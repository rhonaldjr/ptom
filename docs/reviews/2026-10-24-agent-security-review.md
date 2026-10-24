# Persona review: agent, server and discovery (M4–M6)

Date: 2026-10-24 · Scope: `c0ac926..a16c83a` (remote-access surface) ·
Reviewers: Security Architect, Ethical Hacker (independent subagents, given the code but not each other's findings) ·
Process: [code review](../process/code-review.md)

Both reviewers verified their findings by reading the code. Overlapping findings are merged below.
**All were fixed in the follow-up commit except S11/P-32, which is left for the user.**

| # | Finding | Severity | Fix |
|---|---|---|---|
| 1 | Idle connections fill all 16 slots, so nobody (including the owner) can connect (EH H1, SA 1) | **High** | 15 s login deadline including TLS; at most 8 unadmitted connections overall and 2 per address, so admitted viewers always have room; 30 s write timeout (verified live) |
| 2 | Lockout bypass: IPv6 rotation, parallel guesses, address-table eviction (EH H2) | **High** | IPv6 counted per /64 · IPv4-mapped addresses normalised · global budget (30 failures/min pauses logins for 1 min) · pending-per-address cap limits parallel guessing |
| 3 | A successful but prompt-gated login cleared the failure count (SA 3, EH M2) | Major | Failures are cleared only when a viewer is actually let in (`Connected`) |
| 4 | Unbounded input buffered during the approval prompt, then replayed as keystrokes on Allow (SA 2, EH M3) | Major | Any data sent between `ClientInit` and `ServerInit` closes the connection; unread input is capped at 32 MiB in both state machines |
| 5 | Prompt flooding: an attacker repeats requests until one is accepted (SA 4) | Major | One approval prompt at a time; a denial or timeout counts as a failed login; the prompt names the security type |
| 6 | Wayland approval persisted across logins, beyond U-3's "once per login" (SA 5) | Major | Restore token kept in `$XDG_RUNTIME_DIR` (cleared at logout) |
| 7 | Plaintext left over at a TLS/RA2 upgrade was read as if protected (STARTTLS injection; SA 6, EH M4) | Medium | Both state machines refuse an upgrade with unread input (test) |
| 8 | "Disconnect everyone" missed stuck tasks and allowed instant reconnects (EH M5) | Medium | Tasks are aborted outright; sharing pauses for 2 minutes |
| 9 | Argon2id password checks on async workers (SA 8, EH M1) | Medium | Login polling runs in `block_in_place` |
| 10 | A spoofed discovered computer using a saved computer's name looked like a normal new computer (EH M6) | Medium | The Home list and tray warn when a discovered name matches a saved computer at a different address |
| 11 | The agent advertises on every network it joins (SA 11) | Minor | **Fixed** after the user's decision on P-32: the agent announces only on networks marked as theirs (ADR-0019) |
| 12 | VncAuth/None and `--allow-legacy` never took effect, contradicting the docs (SA 7) | Minor | Implemented as documented; `SecurityPolicy` still gates them |
| 13 | systemd unit made credentials writable and failed on a fresh install (SA 9, 10) | Minor | `ReadWritePaths=-%h/.config/farsight/agent -%t/farsight` |
| 14 | An unapproved viewer counted as "admitted" for exclusive-access checks (SA 12) | Minor | `AccessControl::started` marks admission only once `ServerInit` is sent |
| 15 | `accept()` errors stopped the agent (EH L1) | Low | Logged and retried |
| 16 | View-only viewers received the desktop clipboard (EH L2) | Low | Clipboard is not sent to view-only viewers |
| 17 | Unbounded list of discovered computers (EH L3) | Low | Capped at 256 |
| 18 | The identity was silently regenerated if one file was unreadable (EH L4) | Low | Generated only when both files are absent; otherwise an error; directory permissions re-tightened (test) |

Confirmed sound by the reviewers: view-only enforcement, rejecting messages before `ServerInit`, deny-by-default prompts,
owner detection only from verified usernames, isolation between viewers, VeNCrypt credential length caps,
`X509None` never offered, and the prober sending only a version message.
