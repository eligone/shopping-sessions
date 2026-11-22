# ADR-043: Persons map without identity entries; prefetch in the clicks lookup

**Status:** Accepted (branch `lowlevel`)
**Date:** 2026-11-22

## Context
Every event and click maps its `user_id` to a person through the persons table
(`links::persons`). Every lookup already falls back to the ID itself on a miss
(`persons.get(user).unwrap_or(user)`), yet the table also stored every linked ID
whose component name is itself, and was sized at about 2 × the IDs (load ≤ 0.5).
The clicks loader looked up each user right after parsing it, with no prefetch, so
each lookup could wait on a cache miss.

## Decision
- Store only IDs whose person differs from themselves. Size the table for a load of
  about 0.75 (capacity next_pow2(4m/3) for m entries, grown if needed).
- In the clicks loader, parse a chunk into a temporary vector first, then do the
  person lookups with the slot prefetched 16 records ahead (as events already do, ADR-028).

## Why it is exact
A missing key returns the ID itself, which is exactly what the dropped identity
entries returned. Routing and all other values are unchanged.

## Measurements (AWS m5.2xlarge, 20M, 5 alternating runs)
| Binary | Median |
|---|---|
| ADR-042 (`40c07ee`) | 2.136 s |
| ADR-043 | 2.116 s |
Faster in every run. `scripts/check.sh full`: all checks passed.
