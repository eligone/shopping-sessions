# ADR-025: Flat open-addressing table for dedup, products, campaigns and persons

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-21

**Context.** After [ADR-023](023-u64-ids-u32-times.md) every hot key is a non-zero u64. The dedup
closure was about 10% of CPU on Linux ([ADR-021](021-linux-profiling.md)), and every event does a
product lookup and a user→person lookup, all through `std::collections::HashMap`, which is general
purpose (any key type, deletion, SIMD control bytes).

**Decision.** `src/table.rs` `FlatMap<V>`: two flat arrays (keys, values), power-of-two capacity at
2× the expected size, linear probing, key 0 marks an empty slot (no ID's key is ever 0), no
deletion, and growth at 3/4 full so inserts always terminate. The slot comes from the low bits of
`ids::mix`; partition buckets use bits 28..36, so the two don't correlate. Used for:
- event dedup per bucket (`upsert` keeping the smallest ingest_ts);
- products.csv and campaigns.csv lookups (`insert_new`: the first row wins, as before);
- the user→person map from links.

The links interning map and the byte-keyed maps for escaped IDs stay as `HashMap`.

**Verification.**
- `table::tests::fuzz_matches_std_hashmap`: 20 rounds × 50k random upserts, first-wins inserts and
  lookups, from tiny and from large capacities (so growth is exercised), compared with std's
  `HashMap`, including final contents.
- Mutation check: a lookup that stops at the first occupied slot makes the test fail.
- `scripts/check.sh`: 12/12 unit tests, fixtures in both modes, dev MATCH, seeds 2–3 MATCH.
- Not benchmarked locally; the scorer measures.
