# ADR-027: Mark dedup winners in a bitset instead of sorting their locations

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-21

**Context.** After dedup, each bucket collected its winners' locations from the table, read each
winner's `valid` flag at a random place in memory, then sorted all locations (about 20M in
total, by comparison) so the records could be read back in memory order. The research scout
estimated this at 0.08–0.15 s at 20M.

**Decision.** Per bucket, give each record an index across all chunks (a prefix sum of the
per-chunk counts), set one bit per winner in a bitset (n/8 bytes, cache-sized), then scan the
records in their natural order and route those whose bit is set and that are valid. The output
order is the same chunk-then-position order the sort produced. No random `valid` reads, no sort.

**Verification.** `scripts/check.sh`: 12/12 unit tests, fixtures in both modes, dev MATCH, seeds 2–3
MATCH. The mutation test's dedup-order mutation still targets the unchanged comparison.
