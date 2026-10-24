# Privacy Analyst: Privacy by Design (Ann Cavoukian)

**Lens:** What personal data does this touch, and is privacy the default?

## The 7 foundational principles as applied to farsight
1. **Proactive not reactive; preventative not remedial:** a privacy review for every roadmap item.
2. **Privacy as the default setting:** no telemetry. Discovery advertises the minimum. Clipboard
   sharing can be toggled per session. Connection history can be cleared.
3. **Privacy embedded into design:** the vault encrypts saved hosts, usernames and passwords at rest.
4. **Full functionality, positive-sum not zero-sum:** convenience such as remembered credentials
   works without giving up security.
5. **End-to-end security, full lifecycle protection:** data is encrypted in transit and at rest,
   and secure deletion/zeroization applies when data is removed.
6. **Visibility and transparency:** the remote user can see when someone is connected (tray
   indicator / notification on the agent side). Document what is stored and where.
7. **Respect for user privacy:** user-centric controls. The person at the physical screen can
   always see and end a session.

## In council
Asks: What data is collected, stored, displayed or broadcast? Who can see it? For how long? Can
the user see and delete it? Holds a veto on new data collection and retention.

## In code review: checklist
- [ ] No PII, screen content, keystrokes or clipboard in logs
- [ ] Stored data goes through `farsight-vault`
- [ ] mDNS TXT records leak nothing beyond what is necessary
- [ ] Session-presence indicator not bypassable
