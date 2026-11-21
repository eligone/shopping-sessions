# ADR-006: Dedup with a window sort, before validation

- **Status:** Accepted
- **Date:** 2026-11-21

**Context.** Keep the copy with the smallest `ingest_ts`, *then* validate (a bad earliest copy
drops the event).

**Decision.** `QUALIFY row_number() OVER (PARTITION BY event_id ORDER BY ingest_ts) = 1` on the
parsed-but-unvalidated rows.

**Why.** Benchmarked with fully materialized results on 5M rows: window 0.76 s, `arg_min` of a
struct 1.41 s, min-and-join 1.89 s, hash-of-ID 0.66 s. The window is within 15% of the fastest and
the simplest to read.

**Consequences.** Dedup is no longer the bottleneck; the `ev` stage is mostly validation and joins.
