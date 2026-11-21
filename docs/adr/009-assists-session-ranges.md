# ADR-009: Assists via merged click intervals and ASOF-found session ranges

- **Status:** Accepted
- **Date:** 2026-11-21

**Context.** A campaign assists a session if any of its clicks falls in `[start - 7d, start]`,
which is the same as `start` falling in `[click, click + 7d]`. A shared-device "person" can have
tens of thousands of sessions and clicks, so joining every session to every click of the same
person grows quadratically.

**Decision.**
1. Per `(person, campaign)`, merge click intervals whose gap is at most 7 days into disjoint
   ranges `[lo, hi]`.
2. Number each person's sessions by start (starts are distinct within a person). Two ASOF lookups
   turn each range into a contiguous run of session numbers `[r1, r2]`.
3. Join each run to the person's per-day session-number ranges (at most about 15 days) and add up
   the overlaps.

**Why.** Cost grows with the number of click ranges and sessions, not their product, so a
pathological shared-device person stays cheap.

**Consequences.** More SQL than a plain range join, but the same answer (fixtures and dev set
match) with no quadratic blow-up.
