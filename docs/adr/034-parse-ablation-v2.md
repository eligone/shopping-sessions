# ADR-034: Re-measure the events parse on the new data, with the current scanner

- **Status:** Accepted (branch `partitioned`)
- **Date:** 2026-11-22

**Context.** `SS_STAGE=parsebench` ([ADR-017](017-parse-ablation.md)) still used the old per-line field
splitter and lumped all ID work together. The data now has long IDs, and the parser has changed
(scanner, wide event IDs, direct product index).

**Decision.** The benchmark now uses `csv::for_each_line`, and separates `ts`, ingest/price/type, the
event_id wide key, user_id packing (which interns long IDs) and the product lookup. The old
`csv::fields` path is kept only as a fuzz-test oracle.

**Results (new 1M dev set, 8 threads, Mac, rounds 1–2; full parse about 0.048 s).**

| Step | Cost | Share |
| --- | --- | --- |
| scan lines + fields | 0.021 s | 44% |
| `ts` | +0.004–0.009 s | about 13% |
| ingest, price, type | +0.001 s | 2% |
| event_id wide key | +0.005–0.009 s | about 15% |
| user_id pack (interning) | +0.007–0.009 s | about 17% |
| product lookup | about 0 | the direct index works ([ADR-032](032-direct-index-lookups.md)) |
| bucket push | +0.008 s | about 17% |

**Consequences.** Next: a per-thread cache in front of the interner for long user IDs, which repeat
about 30 times per person; then scanner tuning; then the output writer.
