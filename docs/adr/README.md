# Architecture Decision Records

Decisions behind `pipeline/pipeline.py`, the Shopping Sessions pipeline. `rules.md` is the spec;
these records explain *why* the pipeline is built the way it is. One decision per file.

| ADR | Decision | Status |
| --- | --- | --- |
| [001](001-duckdb-engine.md) | Use DuckDB as the engine | Accepted |
| [002](002-hand-rolled-csv-parsing.md) | Parse CSV lines ourselves, not with DuckDB's CSV dialect handling | Accepted |
| [003](003-fast-reader-with-cr-fallback.md) | Fast line reader with an exact fallback for files containing `\r` | Accepted |
| [004](004-strict-timestamp-parsing.md) | Strict timestamp parsing as one SQL macro, returning NULL when bad | Accepted |
| [005](005-linked-ids-connected-components.md) | Linked IDs via connected components, named by the smallest index | Accepted |
| [006](006-dedup-window-sort.md) | Dedup with a window sort, before validation | Accepted |
| [007](007-range-frames-for-ties.md) | Sessionize with RANGE window frames so tied timestamps always share a session | Accepted |
| [008](008-credit-asof-join.md) | Credit with an ASOF join over tie-collapsed clicks | Accepted |
| [009](009-assists-session-ranges.md) | Assists via merged click intervals and ASOF-found session ranges | Accepted |
| [010](010-verification-and-safe-local-runs.md) | Verify against fixtures, the dev answer and repeat runs; keep runs safe on small machines | Accepted |
| [011](011-commit-conventions.md) | Commit conventions: no AI co-authors, ADRs updated with every commit | Accepted |
| [012](012-rust-rewrite.md) | Rewrite the pipeline in Rust, with integer IDs and one parse pass | Accepted |
| [013](013-readahead-hints.md) | Ask the kernel to read every input file ahead (`madvise WILLNEED`) | Accepted |
| [014](014-pread-read-mode.md) | Parallel `pread` read mode, selectable against mmap | Accepted (branch `rust`) |
| [015](015-fewer-event-copies.md) | Fewer copies of each event: validate at parse, dedup by location, fill one output | Accepted (branch `rust`) |
| [016](016-parallel-concat.md) | Parallel concat for per-task results (clicks) | Accepted (branch `rust`); unproven locally |
| [017](017-parse-ablation.md) | Measure the events parse by ablation before optimizing it | Accepted (branch `rust`) |
| [018](018-bitmask-field-splitting.md) | Bitmask field splitting, guarded by a differential fuzz test | Accepted (branch `rust`) |
| [019](019-mutation-testing.md) | Mutation testing: plant realistic bugs and check the tests catch them | Accepted |
| [020](020-partitioned-pipeline.md) | Partitioned pipeline: dedup by event_id bucket, then work per person partition | Accepted (branch `partitioned`) |
| [021](021-linux-profiling.md) | Profile on Linux with perf in Docker | Accepted |
| [022](022-huge-pages.md) | Ask mimalloc for huge OS pages (`MIMALLOC_ALLOW_LARGE_OS_PAGES=1`) | Accepted (branch `partitioned`); to be scored |
| [023](023-u64-ids-u32-times.md) | 64-bit base-37 IDs and 32-bit event times | Accepted (branch `partitioned`) |
| [024](024-chunk-scanner.md) | One-pass chunk scanner with 64-byte bitmasks (AVX2 on x86) | Accepted (branch `partitioned`) |
| [025](025-flat-hash-table.md) | Flat open-addressing table for dedup, products, campaigns and persons | Accepted (branch `partitioned`) |
| [026](026-links-alongside-parse.md) | Resolve links while events are parsed | Accepted (branch `partitioned`) |
| [027](027-winner-bitset.md) | Mark dedup winners in a bitset instead of sorting their locations | Accepted (branch `partitioned`) |
| [028](028-paired-slots-prefetch.md) | FlatMap slots hold key and value together; prefetch upcoming lookups | Accepted (branch `partitioned`) |
| [029](029-sharded-interner.md) | Shard the escaped-ID interner for the generator's long IDs | Accepted (branch `partitioned`) |
| [030](030-cheaper-cmp-ids.md) | Cheaper byte-wise ID comparison (`cmp_ids`) | Accepted (branch `partitioned`) |
| [031](031-wide-event-ids.md) | Exact wide keys for event_id: no interning for long event IDs | Accepted (branch `partitioned`) |
| [032](032-direct-index-lookups.md) | Direct-index product and campaign lookups; fast scanner for the small CSVs | Accepted (branch `partitioned`) |
| [033](033-mmap-default.md) | Read inputs through mmap on the scorer (`SS_READ=mmap` in run.sh) | Accepted (branch `partitioned`) |
| [034](034-parse-ablation-v2.md) | Re-measure the events parse on the new data, with the current scanner | Accepted (branch `partitioned`) |
| [035](035-thread-cache-long-ids.md) | Per-thread cache in front of the interner for long hex-shaped IDs | **Rejected**: scored slower (2.51 s), removed |
| [036](036-shared-buffer-output.md) | Write user_daily strings from one shared buffer | Accepted (branch `partitioned`) |

To add one: copy the next number, write Context / Decision / Why / Consequences, and add a row above.

## Targets (from the track page)

| Run | Data | Limit | Goal |
| --- | --- | --- | --- |
| Dev check | 1M events (seed 1) | 3 min | must match exactly |
| Practice | about 20M events | 2x baseline (171 s), never over 5 min | aim under about 60 s |
| Final | about 60M events | 3 min per run, all 3 runs must finish | median is the score |

Score is run time from container start to exit, lower is better.

## Open questions

- **Rules audit (agent, `60c7ade`):** no input valid under rules.md found that the code gets wrong.
  Speculative notes:
  - peak memory at 60M: measured 0.91 GB at 5M and 1.63 GB at 10M, so about 10 GB at 60M against a
    24 GB limit: OK;
  - stale `part-N.parquet` files if `/output` were reused (the scorer gives an empty one);
  - hidden `.x.csv` files would be read;
  - u16 campaign and category indices would wrap above 65,535 distinct values (the spec says
    2,000 and 20).
- **Research scout (agent):** top ideas are huge pages for heap memory, u64 IDs plus u32 relative
  times (records 56/32 B down to about 32/16 B), whole-block AVX2 structural masks with no
  per-line copy, cache-sized custom dedup and product tables, and per-partition radix sort.

- **Second submission (`c27152b`, DuckDB): scored 37.63 s** on about 20M events.
- **Third submission (`c44891f`, Rust + mmap/`WILLNEED`, option A): scored 4.06 s**, best so far.
  Saved on branch `option-1`.
- **Fourth submission (`3b5c1d3`, Rust + pread, option B): scored 4.05 s**, a tie with A within
  scorer noise (about 20%). Reading mode is not the bottleneck on the scorer.
- **Fifth submission (`1f93dbc`, ADR-015/016/018): scored 3.80 s.** The leaderboard top is 0.28 s.
- **Sixth submission (`da351f1`, partitioned pipeline + mimalloc, ADR-020): scored 3.02 s.**
- **Seventh submission (`55032eb`, huge pages + u64 IDs + chunk scanner, ADR-022/023/024): scored 2.67 s**,
  It also confirms the AVX2 path builds and is correct on the scorer.
- **Eighth submission (`c203fa6`, FlatMap, ADR-025): scored 2.45 s**, best so far. Reviewed by the rules
  auditor first (no issues).
- **Ninth submission (`87780b9`, batch ADR-026/027/028): scored 6.63 s** (Sun 12:01 AM). The leaderboard top
  also moved from 0.28 s to 0.51 s, so the scorer itself appears slower (about 1.8x); the batch may also have
  regressed. Next: resubmit `c203fa6` unchanged to measure the drift. Rules now say 2 submissions an hour.
- **Tenth submission (`c203fa6` resubmitted): 6.94 s.** So the batch is about 5% faster, and the data changed:
  rules.md now says 1 in 8 IDs carries a 32-char hex suffix. Our single-lock escaped-ID table was the
  bottleneck (ADR-029).
- **Eleventh submission (`703f333`, batch + sharded interner, ADR-026..029): scored 2.62 s** on the new data,
  (6.63 s before the interner fix). Leaderboard top: 0.51 s.
- **Twelfth submission (`f06e24e`, ADR-030 cheaper cmp_ids + ADR-031 wide event IDs): scored 2.46 s**, best so far
  (−0.16 s, inside the scout's 0.15–0.25 s estimate).
- **Thirteenth submission (`7f9f4ca`, ADR-032 direct-index lookups + ADR-033 mmap): scored 2.40 s**, best so far
  (−0.06 s, within noise but in the expected direction).
- **Fourteenth submission (`bb03de6`, ADR-035 thread cache + ADR-036 output buffer): 2.51 s**, slower than 2.40 s.
  Suspect: the cache (extra work per ID, and a 1 MB table competing for a 1 MB L2). Cache removed.
- **First submission (`42c1d15`) ran past the practice limit** on about 20M events. The likely
  cause is the quadratic assists join on shared-device persons, removed in [ADR-009](009-assists-session-ranges.md). Current
  timing: about 7 s for 5M events on 4 laptop cores.
- **Next speed candidates:** the `ev` stage (validation and joins, about 2.7 s at 5M), and keeping
  string IDs out of the large sorts.
- **Rust branch (`rust`):** 36/36 fixtures, dev set MATCH, identical to DuckDB on seeds 2-10 and 5M;
  0.24 s on the dev set, 1.34 s on 5M (4 laptop threads). Not yet submitted.
- **Docker image not yet built locally** (Docker Desktop was not running).

## Changelog

One line per commit, newest last.

- 2026-11-21: DuckDB pipeline, fixtures pass, dev set match (main).
- 2026-11-21: Tie fix, faster parsing, SciPy links, linear assists: scored 37.63 s (main, `c27152b`).
- 2026-11-21: ADRs split into files; CLAUDE.md commit rules (ADR-011).
- 2026-11-21: Rust pipeline on branch `rust` (ADR-012): fixtures, dev and 10-dataset diff all match.
- 2026-11-21: Events sub-stage timers; events and clicks timed separately. Found warm vs cold
  file cache explains half the 5M time (1.35 s cold, 0.70 s warm). CLAUDE.md: commit often.
- 2026-11-21: Read-ahead hints on all input files, events and clicks concurrent again (ADR-013).
  Rust merged into `main` for submission.
- 2026-11-21: Option B in progress: `pread` read mode (default) next to `mmap` (`SS_READ=mmap`);
  line-ownership boundary test for every chunk size. Fixtures and dev MATCH in both modes.
- 2026-11-21: ADR-014: pread vs mmap measured (pread about 25% faster cold, about 10% slower warm); 5M
  pread output identical to DuckDB.
- 2026-11-21: Option A (`c44891f`) scored 4.06 s. Saved as branch `option-1`; `main` moves to the
  pread version (option B) for the next submission.
- 2026-11-21: Option B (`3b5c1d3`) scored 4.05 s, a tie with A: next target is CPU work, not I/O.
- 2026-11-21: `scripts/check.sh` runs every correctness check in one command (`full` for 9 seeds and 5M).
- 2026-11-21: Sub-stage timers for links, clicks, sessions, output. 10M profile: memory traffic and
  first-touch page faults dominate (252k page reclaims, about 4 GB touched, 1.35 s system time).
- 2026-11-21: ADR-015: events validated at parse, deduped by location, built into one output; RawEv 56 B.
  Dedup about 40% faster, about 25% fewer page reclaims at 10M.
- 2026-11-21: ADR-016: `src/par.rs` `par_concat`, used by clicks. No measurable local change: the laptop's
  memory pressure (6 GB swap) now exceeds the effects, so the scorer measures from here.
- 2026-11-21: ADR-017: `SS_STAGE=parsebench` ablation. Field splitting is 35% of parse (the largest
  single cost); expected gain from SIMD about 25% of parse, about 8% of the run.
- 2026-11-21: ADR-018: bitmask `csv::fields` (scalar kept as reference), fuzz test with mutation check.
  Events parse 0.61 -> 0.53 s at 10M on the Mac (ARM); may differ on x86.
- 2026-11-21: ADR-019: `scripts/mutation_test.py`; 12/15 caught, the 3 survivors are equivalent mutants.
- 2026-11-21: Fixed check.sh hiding failures behind pipes, and the mutation script leaving a stale build;
  full check passes on `edff39d`'s code.
- 2026-11-21: `1f93dbc` scored 3.80 s (best). Leaderboard top is 0.28 s.
- 2026-11-21: Branch `partitioned`: 256 eid buckets -> 256 person partitions; per-partition sort, sessions,
  attribution (no global sort). Buckets from the middle of the hash. All checks pass.
- 2026-11-21: mimalloc as the global allocator: 10M at 8 threads 1.75-2.11 -> 1.40-1.67 s, about 15% fewer page reclaims.
- 2026-11-21: ADR-020 written; exit without destructors; 4 MB chunks confirmed best (2/4/8/16 MB measured).
- 2026-11-21: `da351f1` (partitioned) scored 3.02 s, 20% better than 3.80 s.
- 2026-11-21: Rules audit and performance research by agents recorded; peak memory 1.63 GB at 10M.
- 2026-11-21: ADR-021: Linux profiling (Dockerfile.profile, scripts/linux_profile.sh). Parsing is about 57% of CPU;
  THP already `always` in Docker's VM.
- 2026-11-21: ADR-022: run.sh sets MIMALLOC_ALLOW_LARGE_OS_PAGES=1; only run.sh differs from main's code (clean A/B).
- 2026-11-21: CLAUDE.md: heavy work (benchmarks, tests, generation, Docker) moves off the laptop to a cloud VM.
- 2026-11-21: ADR-023: u64 base-37 IDs (escape bit for others), u32 event times; RawEv 40 B, Ev 24 B.
  check.sh builds with -j 2 to stay light on the laptop.
- 2026-11-21: ADR-024: one-pass chunk scanner (AVX2 masks on x86) for events, clicks, links; fuzz-tested.
- 2026-11-21: `55032eb` scored 2.67 s (12% better than 3.02 s); main moves to it, 3.02 s saved as `scored-3.02`.
- 2026-11-21: ADR-025: `src/table.rs` FlatMap for dedup, products, campaigns, persons; fuzz-tested against HashMap.
- 2026-11-21: `c203fa6` (FlatMap) scored 2.45 s, 8% better than 2.67 s; main moves to it, 2.67 s saved as `scored-2.67`.
  Second rules audit (changes since 60c7ade): no issues. Scout reassessment: next are links in parallel, a winners
  bitset, paired FlatMap + prefetch, and a per-partition counting sort.
- 2026-11-21: ADR-026: links run alongside events parse (events::parse / events::dedup_route split).
- 2026-11-21: ADR-027: dedup winners marked in a bitset; records read in order (no 20M-location sort).
- 2026-11-21: ADR-028: FlatMap (key, value) slots; prefetch 16 ahead in dedup and route.
  Batch for one submission: ADR-026, 027, 028.
- 2026-11-22: batch `87780b9` scored 6.63 s; leaderboard top 0.28 -> 0.51 s. Plan: resubmit `c203fa6` to separate
  scorer drift from a regression before bisecting ADR-026/027/028.
- 2026-11-22: rules.md update (long hex-suffixed IDs) found; ADR-029 sharded interner, 2.2-2.5x faster on
  simulated long-ID data, matches DuckDB.
- 2026-11-22: New generator and dev answer installed; dev set regenerated; all checks pass on the new data.
- 2026-11-22: `703f333` scored 2.62 s on the new long-ID data (from 6.63 s); main moves to it, the old main is
  saved as `scored-2.45-olddata`.
- 2026-11-22: ADR-030: cmp_ids with an equality short-circuit and no allocation for short vs escaped (auditor notes).
- 2026-11-22: ADR-031: exact wide event_id keys (long hex IDs packed into 128 bits), generic Table; RawEv 48 B.
- 2026-11-22: CLAUDE.md: explain decisions and concepts in plain language for the owner (an undergrad).
- 2026-11-22: `f06e24e` scored 2.46 s (from 2.62 s); main moves to it, 2.62 s saved as `scored-2.62`.
- 2026-11-22: ADR-032 direct-index product/campaign lookups (NumIndex), ADR-033 mmap in run.sh. First small batch.
- 2026-11-22: `7f9f4ca` scored 2.40 s; main moves to it, 2.46 s saved as `scored-2.46`.
- 2026-11-22: ADR-034: parse ablation v2 on new data (scanner 44%, user interning 17%, product lookup ~0).
- 2026-11-22: ADR-035: thread-local cache for long hex user IDs (pack_once for click IDs). No measurable local gain.
- 2026-11-22: ADR-035 revised: direct-mapped, L2-sized (2^15 slots) thread cache, per the scout.
- 2026-11-22: ADR-036: user_daily strings from one shared Bytes buffer, names unpacked once; output 2x faster locally.
- 2026-11-22: `bb03de6` scored 2.51 s (worse); ADR-035 cache removed, ADR-036 kept for a clean A/B against `7f9f4ca`.
- 2026-11-22: .gitignore also blocks SSH/key files (*.pem, *.key, id_*, hackathon-aws*).
