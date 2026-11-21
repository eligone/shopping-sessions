# ADR-028: FlatMap slots hold key and value together; prefetch upcoming lookups

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-21

**Context.** [ADR-025](025-flat-hash-table.md)'s FlatMap kept keys and values in separate arrays, so
a hit cost two cache misses. The dedup table and especially the user→person map (about 64 MB at
20M events) are looked up at effectively random places, so each lookup waits about 100 ns for
DRAM. The research scout estimated 0.1–0.25 s for pairing the slots plus software prefetch.

**Decision.**
- **One array of `(key, value)` slots,** so a lookup touches one cache line.
- **`FlatMap::prefetch(k)`** issues `_mm_prefetch::<_MM_HINT_T0>` for `k`'s first slot on x86-64,
  and is a no-op elsewhere. A prefetch is only a hint and cannot change results.
- **Prefetch 16 records ahead** in the dedup loop (the dedup table) and in the route loop (the
  person map, for valid records). Their slots load while the current records are processed, so
  the DRAM waits overlap.

**Verification.** `scripts/check.sh`: 12/12 unit tests (the FlatMap fuzz test against std HashMap
covers the new layout), fixtures in both modes, dev MATCH, seeds 2–3 MATCH.

**Risk.** As with ADR-024, the x86 prefetch line is compiled only on the scorer; the scorer's
dev-set check guards correctness.
