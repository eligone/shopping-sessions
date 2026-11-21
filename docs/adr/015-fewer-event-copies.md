# ADR-015: Fewer copies of each event: validate at parse, dedup by location, fill one output

- **Status:** Accepted (branch `rust`)
- **Date:** 2026-11-21

**Context.** Profiling at 10M events (ADR-014 timers) showed memory traffic, not parsing logic,
dominating: about 4 GB of freshly allocated memory touched per run (252k page reclaims, 1.35 s of
system time). Each event was copied four times: parsed `RawEv` (64 B) into a dedup bucket, the
winner copied into `kept` (64 B), converted into a per-bucket `Ev` (32 B), and copied again into
one output vector (32 B).

**Decision.**
1. **Validate each copy while parsing.** The rules validate the copy that wins dedup; a copy's
   validity depends only on its own fields, so it is computed once at parse time and stored as a
   `valid` flag. Dedup order is unchanged: dedup still chooses among all copies, valid or not.
2. **Dedup by location.** Each bucket's hash map stores where the best copy is
   (`chunk << 32 | position`, 8 bytes) instead of a copy of it. The valid winners' locations are
   sorted so they are read back in memory order.
3. **One exactly-sized output.** Count the winners per bucket, allocate the output once, and let
   each bucket write its own disjoint slice in parallel. There is no final concat.
4. **`RawEv` from 64 to 56 bytes**, by storing IDs as `[u64; 2]` (8-byte alignment) instead of
   `u128` (16-byte alignment, which padded the struct). A unit test pins the size.

**Verification.** `scripts/check.sh`: 7/7 unit tests, 36/36 fixtures in both read modes, dev MATCH,
diff MATCH on seeds 2–3.

**Measurements (10M events, 4 threads, macOS under memory pressure, 3 alternating runs).**

| | Old | New |
| --- | --- | --- |
| events dedup | 0.47–0.55 s | 0.26–0.34 s |
| concat / build | 0.15–0.19 s | 0.11–0.22 s |
| page reclaims | 244k–255k | 170k–205k |
| system time | 1.26–1.46 s | 0.91–1.29 s |

**Consequences.** Event parsing (about 1 s at 10M) is now the largest stage: the CSV scan plus
writing 56 B per line into the buckets.
