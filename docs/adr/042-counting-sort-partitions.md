# ADR-042: Group each partition by person with a counting sort

- **Status:** Accepted (branch `lowlevel`)
- **Date:** 2026-11-22

**Context.** Partitions took 0.45 s at 20M on the Intel VM, with comparison sorts at about 11% of CPU. Each partition gathered its pieces into one Vec (a copy), then sorted by (person, ts); clicks the same, by (person, ts, click_id).

**Decision.** `partition::group_partition`:
1. give each person a dense local id (a small FlatMap, first-seen order) and count per id;
2. scatter every event straight to its final offset (no gather copy);
3. sort each person run by ts (insertion sort up to 32, `sort_unstable_by_key` above).

Clicks get the same ids; each run is sorted by (ts, click_id) and identical copies are dropped in place. `attribution::walk` takes a per-person click lookup instead of walking two lists in person-key order.

**Verification.** `partition::tests::fuzz_group_partition_matches_sort` compares against the old gather + sort + dedup (equal timestamps, duplicate clicks, persons with only clicks, contiguous runs). Full check on the Intel VM passes (19/19).

**Measurement (VM, 20M, 5 alternating runs).** Median 2.233 s → **2.151 s** (−0.08 s); faster in all 5.
