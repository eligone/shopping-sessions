# ADR-017: Measure the events parse by ablation before optimizing it

- **Status:** Accepted (branch `rust`)
- **Date:** 2026-11-21

**Context.** Plan B is to speed up CSV field splitting with SIMD. Before rewriting the most
delicate code, confirm that field splitting is where the parse time actually goes. A sampling
profiler cannot answer this: LTO inlines `csv::fields`, `ts::parse_ts` and `ids::pack` into
`parse_chunk`, so they don't show up as separate functions.

**Decision.** A development-only benchmark, `SS_STAGE=parsebench`, runs the events parse loop six
times, each variant adding one piece of work, and prints each variant's time and throughput. The
difference between consecutive variants is the cost of that piece (ablation). The normal run is
unaffected.

**Results (10M events, 815 MB of event CSV, 4 threads; rounds 0 and 2; round 1 was disturbed
by memory pressure).**

| Variant | Time | Cost of the added step | Share |
| --- | --- | --- | --- |
| 1 read + split lines | 0.13 s | 0.13 s | 21% |
| 2 + split fields | 0.35 s | +0.22 s | **35%** |
| 3 + timestamps, price, type | 0.42 s | +0.06 s | 10% |
| 4 + pack IDs, product lookup | 0.51–0.61 s | +0.09–0.20 s | 15–30% |
| 5 + bucket hash | 0.55–0.62 s | about +0.03 s | about 5% |
| 6 + bucket push (full parse) | 0.60–0.62 s | about +0.03 s | about 5% |

**Consequences.**
- Field splitting is the largest single cost, so SIMD field splitting (plan B) is the right target.
- Line splitting already runs at about 6 GB/s including the reads; leave it alone.
- Expected gain from B: about 0.15 s of the 0.6 s parse (about 25%), which is about 8% of the
  whole 10M run. ID packing and the product lookup (step 4) are the next candidate after that.
