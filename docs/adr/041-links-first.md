# ADR-041: Parse links first on all threads; union-find alongside the events parse

- **Status:** Accepted (branch `lowlevel`)
- **Date:** 2026-11-22

**Context.** On the Intel VM at 20M, links parse took 1.08 s for 24 MB, because its tasks were queued behind events tasks in the shared thread pool. Its single-threaded union-find (0.19 s) could only start afterwards, which made links the critical path (1.27 s for links + events, against 1.07 s for events alone).

**Decision.** `links::parse_edges` runs first, on all threads, then `rayon::join(links::persons(edges), events::parse)`.

**Verification.** Full check on the Intel VM passes.

**Measurement (VM, 20M, 5 alternating runs).** Links parse 1.08 s → 0.045 s; median 2.390 s → **2.231 s** (−0.16 s); faster in all 5.
