# ADR-007: Sessionize with RANGE window frames so tied timestamps always share a session

- **Status:** Accepted
- **Date:** 2026-11-21

**Context.** A new session starts when the gap to the previous event is over 30 minutes, and
events with equal timestamps must share a session. The first version used `lag` plus a running
`sum(is_new)` with a **ROWS** frame. With ties at a boundary, DuckDB may order tied rows
differently in the two window passes, so one tied event could land in the previous session.
Found because two runs on a 5M dataset disagreed on a shared-device person (4,879 vs 4,877
events on one day). Fixture E16 covers this case but passed by luck.

**Decision.** Use a **RANGE** frame for the running sum (and for click-interval merging in
[ADR-009](009-assists-session-ranges.md)). RANGE includes all peers with the same timestamp, so every tied row gets the same
session number.

**Consequences.** Output is identical across repeated runs. This was a correctness bug, not just a
speed issue.
