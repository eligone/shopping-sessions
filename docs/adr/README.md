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

To add one: copy the next number, write Context / Decision / Why / Consequences, and add a row above.

## Targets (from the track page)

| Run | Data | Limit | Goal |
| --- | --- | --- | --- |
| Dev check | 1M events (seed 1) | 3 min | must match exactly |
| Practice | about 20M events | 2x baseline (171 s), never over 5 min | aim under about 60 s |
| Final | about 60M events | 3 min per run, all 3 runs must finish | median is the score |

Score is run time from container start to exit, lower is better.

## Open questions

- **Second submission (`c27152b`, DuckDB): scored 37.63 s** on about 20M events.
- **Third submission (`c44891f`, Rust + mmap/`WILLNEED`, option A): scored 4.06 s**, best so far.
  Saved on branch `option-1`.
- **Fourth submission (`3b5c1d3`, Rust + pread, option B): scored 4.05 s**, a tie with A within
  scorer noise (about 20%). Reading mode is not the bottleneck on the scorer.
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
