# ADR-020: Partitioned pipeline: dedup by event_id bucket, then work per person partition

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-21

**Context.** `1f93dbc` scored 3.80 s; the leaderboard top is 0.28 s. The data flow had two
global steps over all events, a dedup table and one sort of all 20M events, both far larger than
the CPU cache, plus one big intermediate events list and a global sessions list.

**Decision.**
1. **Dedup in 256 event_id buckets** (was 64). Each bucket's table is about 80k entries at 20M,
   close to cache-sized.
2. **Route survivors to 256 person partitions** right after dedup. Dedup must come first: the
   rules allow duplicate copies to name different users, so copies of one event could otherwise
   land in different person partitions.
3. **Clicks go straight to the same person partitions while parsing.** Click copies are identical,
   so they land together, and sorting the partition makes them adjacent for dedup.
4. **Per partition, independently and in parallel:** gather, sort events by (person, ts), sort and
   dedup clicks, sessionize, sum person-days, credit and assists into a per-thread
   `CampaignDaily` (a rayon `fold`), then add them up. No global sort and no global sessions list.
5. **Buckets come from the middle of the hash** (`partition::bucket`). Hash tables pick a slot
   from the low bits and keep the top 7 bits as a fingerprint. The old bucket choice used the top
   bits, so all keys in one bucket shared 6 of 7 fingerprint bits and the table's fast rejection
   stopped working.
6. **mimalloc** as the global allocator: it reuses freed memory instead of returning it to the
   OS, which matters with about 65k small routing buffers.
7. **Exit without destructors** once the output is written (`mem::forget` plus
   `process::exit(0)`); the OS reclaims everything at once. No measurable gain on macOS; kept
   because it is harmless and freeing GBs may cost more on Linux.

**Verification.** `scripts/check.sh`: 9/9 unit tests, 36/36 fixtures in both modes, dev MATCH,
seeds 2–3 MATCH; `check.sh full` before merging.

**Measurements (10M events, macOS, memory pressure, alternating runs).**

| | Time |
| --- | --- |
| `main` (`1f93dbc`), 8 threads | 1.73–1.90 s |
| partitioned, 8 threads | 1.45–1.55 s |
| partitioned + mimalloc, 8 threads | 1.40–1.67 s, about 15% fewer page reclaims |

Chunk size, re-measured: 4 MB was fastest in all 3 rounds (against 2, 8 and 16 MB).

**Consequences.** Events parse (about 0.6–0.8 s at 10M) remains the largest stage, then
dedup+route (about 0.3 s) and the partition stage (about 0.25–0.35 s).

**Scored result.** `da351f1` scored **3.02 s** on about 20M events, against 3.80 s for `1f93dbc`
(a 20% gain, more than the 15–25% measured on macOS).
