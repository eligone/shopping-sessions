# ADR-026: Resolve links while events are parsed

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-21

**Context.** The research scout's reassessment (after `c203fa6`, 2.45 s) estimated that the links
stage costs 0.15–0.3 s at 20M events. Its interning, union-find and map building are single-threaded
and random-access, and nothing ran alongside it, so 7 of 8 threads sat idle. But the events parse
does not use the person mapping; only dedup+route does.

**Decision.** `events::load` is split into `events::parse` (no person mapping) and
`events::dedup_route` (uses it). main runs `rayon::join(links::persons, events::parse)`, then
`rayon::join(events::dedup_route, clicks::load)`, so idle threads help whichever job is still
running. No rule logic changes.

**Verification.** `scripts/check.sh`: 12/12 unit tests, fixtures in both modes, dev MATCH, seeds 2–3
MATCH. A single change from `c203fa6`, so its score is a clean A/B.
