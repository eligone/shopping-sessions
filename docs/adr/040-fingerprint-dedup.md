# ADR-040: Cache-sized dedup table with 8-byte fingerprint slots

- **Status:** Accepted (branch `lowlevel`)
- **Date:** 2026-11-22

**Context.** On the Intel VM, events dedup+route took 0.70 s at 20M. Each of the 256 event_id buckets
(about 83k records) used a `WideMap<u64>` with 32-byte slots at 2× capacity, about 8 MB per bucket,
against a 1 MB L2; with 8 threads that's about 64 MB live, more than the 33 MB L3. So nearly every
upsert waited on DRAM. Design by the research scout (low-level review, item A1).

**Decision.** `events::dedup_bucket`: slots are one `u64 = fingerprint << 32 | record index`, where 0
means empty and a fingerprint is 28 high bits of the Wide hash, `| 1` so it is never 0. Capacity is
`next_pow2(n × 8/5)`, load ≤ 0.625: about 1 MB per bucket. On a fingerprint match the actual record
is found by its index (a binary search over the piece starts; only on matches), and the full Wide
event_ids are compared; on equal IDs the smaller ingest_ts wins, the first one seen on ties. Then
one pass over the table sets the winner bitset ([ADR-027](027-winner-bitset.md)). Prefetch 16 ahead as
before.

**Verification.**
- `events::tests::fuzz_dedup_bucket_matches_reference`: 60 rounds, up to 6k records, random pieces
  including empty ones, many ingest ties, and every other round a fingerprint function with only 2
  values (forced collisions), against a `HashMap` reference.
- Mutation checks: picking the later copy on ties, and trusting a fingerprint match without
  comparing IDs, both fail the test.
- Full check on the Intel VM passes (18/18, fixtures, dev answer, seeds 2–10).

**Measurement (Intel VM, 20M, 5 alternating runs).** Median 2.567 s → **2.457 s** (−0.11 s); faster in
4 of 5 runs.
