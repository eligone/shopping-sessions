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

To add one: copy the next number, write Context / Decision / Why / Consequences, and add a row above.

## Targets (from the track page)

| Run | Data | Limit | Goal |
| --- | --- | --- | --- |
| Dev check | 1M events (seed 1) | 3 min | must match exactly |
| Practice | about 20M events | 2x baseline (171 s), never over 5 min | aim under about 60 s |
| Final | about 60M events | 3 min per run, all 3 runs must finish | median is the score |

Score is run time from container start to exit, lower is better.

## Open questions

- **Second submission (`c27152b`): scored 37.63 s** on about 20M events, entered in the final.
- **First submission (`42c1d15`) ran past the practice limit** on about 20M events. The likely
  cause is the quadratic assists join on shared-device persons, removed in [ADR-009](009-assists-session-ranges.md). Current
  timing: about 7 s for 5M events on 4 laptop cores.
- **Next speed candidates:** the `ev` stage (validation and joins, about 2.7 s at 5M), and keeping
  string IDs out of the large sorts.
- **Docker image not yet built locally** with the added SciPy dependency.
