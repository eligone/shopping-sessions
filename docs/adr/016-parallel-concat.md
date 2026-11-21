# ADR-016: Parallel concat for per-task results (clicks)

- **Status:** Accepted (branch `rust`); benefit unproven locally, to be re-checked on the scorer
- **Date:** 2026-11-21

**Context.** Clicks were parsed into one vector per task and then appended into one vector with
`extend_from_slice` in a loop, which is one thread copying about 123 MB at 10M events while the
others wait.

**Decision.** `src/par.rs` adds `par_concat`: allocate the output at its exact final size, split it
into one disjoint slice per input piece, and copy all pieces in parallel. Clicks use it. A unit
test checks order and contents, including empty pieces.

**Verification.** `scripts/check.sh`: 8/8 unit tests, 36/36 fixtures in both modes, dev MATCH, diff
MATCH on seeds 2–3.

**Measurements (10M events, 4 threads, 3 alternating runs; macOS with 6.2 GB of swap in use).**

| Run | Old: parse incl. copy | New: parse + concat |
| --- | --- | --- |
| 1 | 0.92 s | 0.90 s |
| 2 | 0.67 s | 1.14 s |
| 3 | 0.85 s | 0.78 s |

No measurable difference: run-to-run noise (up to 0.5 s) is larger than the effect. The concat
alone took 0.14–0.28 s for 123 MB, about ten times the cost of the copy itself. That is macOS
reclaiming memory from other apps before it can hand out fresh pages, which the scorer (24 GB,
nothing else running) does not pay.

**Consequences.** Kept, because it is correct, tested and removes a single-threaded step that
should matter more with 8 threads. The laptop can no longer resolve differences this small, so
further optimizations are to be judged on the scorer.
