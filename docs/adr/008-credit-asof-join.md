# ADR-008: Credit with an ASOF join over tie-collapsed clicks

- **Status:** Accepted
- **Date:** 2026-11-21

**Context.** Credit the latest valid click at or before the session start, at most 7 days before,
breaking `ts` ties by the smallest `click_id`.

**Decision.** First collapse clicks to one row per `(person, ts)` with
`arg_min(campaign_id, click_id)`, then
`sessions ASOF LEFT JOIN clicks ON person AND start >= click.ts`, then apply the 7-day limit;
anything else is `organic`.

**Why.** ASOF picks "latest at or before" in one ordered pass; collapsing first makes the
tie-break exact instead of whichever row ASOF happens to see.
