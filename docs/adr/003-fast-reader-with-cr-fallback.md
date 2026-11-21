# ADR-003: Fast line reader with an exact fallback for files containing `\r`

- **Status:** Accepted
- **Date:** 2026-11-21

**Context.** The first version split whole files with `read_text` + `string_split` + `unnest`.
Profiling on 5M events: **6.3 s** of a ~15 s run. DuckDB's CSV reader produces the same lines
(one column, `delim=chr(1)`, quoting off) in **0.24 s**, but breaks at `\r` ([ADR-002](002-hand-rolled-csv-parsing.md)).

**Decision.** Scan each file once for `\r` (about 0.2 s for all files). Files without one go
through the fast reader; files with one go through the exact `split-on-\n` path.

**Why.** `rules.md` says the generator never writes `\r`, so real datasets take the fast path,
but correctness does not depend on trusting that.

**Consequences.** Event parsing went from 12.5 s to 1.8 s on 5M events. E20 still passes
through the fallback.
