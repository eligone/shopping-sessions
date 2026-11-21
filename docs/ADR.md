# Architecture Decision Records

Decisions behind `pipeline/pipeline.py`, the Shopping Sessions pipeline.
`rules.md` is the spec; where this file explains *why*, `rules.md` says *what*.

Status of every decision below: **Accepted**.

---

## ADR-001: Use DuckDB as the engine

**Context.** Three starters were offered: DuckDB, Polars, PySpark. The scorer is one machine
(8 vCPUs, 24 GB, no swap), and only run time counts.

**Decision.** DuckDB, driven from Python.

**Why.**
- Most rules are relational: dedup, per-person window ordering, "latest click at or before a
  time". DuckDB has `ASOF JOIN` for that last one built in.
- Vectorized and multi-threaded on one machine, and it spills to `/scratch` under its memory
  limit instead of being killed.
- PySpark carries JVM and cluster overhead that buys nothing on a single box.

**Consequences.** All heavy work is SQL; Python only orchestrates and runs the union-find.

---

## ADR-002: Parse CSV lines ourselves, not with DuckDB's CSV dialect handling

**Context.** `rules.md` defines its own dialect: only `\n` ends a line, `\r` is data, quotes only
around whole fields, `""` is null, wrong field counts are dropped. DuckDB's reader reads `""` as
`''`, repairs or rejects malformed lines its own way, and **splits lines at `\r` even with
`new_line='\n'`** (verified: fixture E20 broke into extra rows).

**Decision.** Read each line as one opaque string, then:
1. keep it only if it fully matches `field(,field){n-1}` where a field is `"[^",]*"` or `[^",]*`;
2. split on `,`, strip quotes, turn empty into NULL.

**Why.** The regex is the dialect, written once, with no reader quirks in between. Empty lines,
stray quotes and wrong field counts all fail the same match.

**Consequences.** Every rule about quoting, spaces and `\r` holds by construction; fixtures
E13, E15, E19, E20 and L3 pass.

---

## ADR-003: Fast line reader with an exact fallback for files containing `\r`

**Context.** The first version split whole files with `read_text` + `string_split` + `unnest`.
Profiling on 5M events: **6.3 s** of a ~15 s run. DuckDB's CSV reader produces the same lines
(one column, `delim=chr(1)`, quoting off) in **0.24 s**, but breaks at `\r` (ADR-002).

**Decision.** Scan each file once for `\r` (about 0.2 s for all files). Files without one go
through the fast reader; files with one go through the exact `split-on-\n` path.

**Why.** `rules.md` says the generator never writes `\r`, so real datasets take the fast path,
but correctness does not depend on trusting that.

**Consequences.** Event parsing went from 12.5 s to 1.8 s on 5M events. E20 still passes
through the fallback.

---

## ADR-004: Strict timestamp parsing as one SQL macro, returning NULL when bad

**Context.** Three valid forms (F1 UTC, F2 offset, F3 epoch ms) and a long list of invalid ones:
impossible dates, hour 24, second 60, bad offsets, non-ASCII digits, fractional values.

**Decision.** `parse_ts(s)`:
- F3: `[0-9]+` only (ASCII by construction); leading zeros stripped; over 15 significant digits is
  treated as out of window instead of overflowing.
- F1/F2: a strict shape regex, then `try_strptime` for calendar validity (it rejects Feb 30,
  Feb 29 in non-leap years, hour 24, second 60; verified), plus explicit offset range checks.
- Anything else is NULL, so it fails validation.

**Why.** One definition used by events and clicks; no `\d`, which matches Unicode digits in some
regex engines.

**Consequences.** `ingest_ts` is always valid F1, so it is parsed with a plain `strptime`, and
for dedup it is compared as text: fixed-width F1 sorts in time order.

---

## ADR-005: Linked IDs via connected components, named by the smallest index

**Context.** IDs must merge transitively (chains hundreds of links long, shared devices joining
thousands of people), and each person is named by the byte-wise smallest ID.

**Decision.** Number the IDs in DuckDB in byte-wise order (`ORDER BY id`; DuckDB compares strings
byte-wise, verified). Run `scipy.sparse.csgraph.connected_components` on the edges. Each
component's smallest index is its smallest ID, so that ID is the name.

**Why.** Recursive SQL for components is slow and can blow up. An earlier NumPy
label-propagation loop worked, but iterates and uses the slow `np.minimum.at`. SciPy runs in
linear time in C.

**Consequences.** New dependencies: `numpy`, `scipy` (pinned, Linux wheels confirmed for the
image's Python 3.12). The label table is handed to DuckDB with an explicit `con.register`, not by
looking up a Python variable name, which broke when run under a profiler.

---

## ADR-006: Dedup with a window sort, before validation

**Context.** Keep the copy with the smallest `ingest_ts`, *then* validate (a bad earliest copy
drops the event).

**Decision.** `QUALIFY row_number() OVER (PARTITION BY event_id ORDER BY ingest_ts) = 1` on the
parsed-but-unvalidated rows.

**Why.** Benchmarked with fully materialized results on 5M rows: window 0.76 s, `arg_min` of a
struct 1.41 s, min-and-join 1.89 s, hash-of-ID 0.66 s. The window is within 15% of the fastest and
the simplest to read.

**Consequences.** Dedup is no longer the bottleneck; the `ev` stage is mostly validation and joins.

---

## ADR-007: Sessionize with RANGE window frames so tied timestamps always share a session

**Context.** A new session starts when the gap to the previous event is over 30 minutes, and
events with equal timestamps must share a session. The first version used `lag` plus a running
`sum(is_new)` with a **ROWS** frame. With ties at a boundary, DuckDB may order tied rows
differently in the two window passes, so one tied event could land in the previous session.
Found because two runs on a 5M dataset disagreed on a shared-device person (4,879 vs 4,877
events on one day). Fixture E16 covers this case but passed by luck.

**Decision.** Use a **RANGE** frame for the running sum (and for click-interval merging in
ADR-009). RANGE includes all peers with the same timestamp, so every tied row gets the same
session number.

**Consequences.** Output is identical across repeated runs. This was a correctness bug, not just a
speed issue.

---

## ADR-008: Credit with an ASOF join over tie-collapsed clicks

**Context.** Credit the latest valid click at or before the session start, at most 7 days before,
breaking `ts` ties by the smallest `click_id`.

**Decision.** First collapse clicks to one row per `(person, ts)` with
`arg_min(campaign_id, click_id)`, then
`sessions ASOF LEFT JOIN clicks ON person AND start >= click.ts`, then apply the 7-day limit;
anything else is `organic`.

**Why.** ASOF picks "latest at or before" in one ordered pass; collapsing first makes the
tie-break exact instead of whichever row ASOF happens to see.

---

## ADR-009: Assists via merged click intervals and ASOF-found session ranges

**Context.** A campaign assists a session if any of its clicks falls in `[start - 7d, start]`,
which is the same as `start` falling in `[click, click + 7d]`. A shared-device "person" can have
tens of thousands of sessions and clicks, so joining every session to every click of the same
person grows quadratically.

**Decision.**
1. Per `(person, campaign)`, merge click intervals whose gap is at most 7 days into disjoint
   ranges `[lo, hi]`.
2. Number each person's sessions by start (starts are distinct within a person). Two ASOF lookups
   turn each range into a contiguous run of session numbers `[r1, r2]`.
3. Join each run to the person's per-day session-number ranges (at most about 15 days) and add up
   the overlaps.

**Why.** Cost grows with the number of click ranges and sessions, not their product, so a
pathological shared-device person stays cheap.

**Consequences.** More SQL than a plain range join, but the same answer (fixtures and dev set
match) with no quadratic blow-up.

---

## ADR-010: Verify against fixtures, the dev answer and repeat runs; keep runs safe on small machines

**Decision.** Every change must pass all of:
- all 36 fixtures (`scripts/run_fixtures.py`);
- an exact match with `dev-answer/` on the 1M-event dev set (seed 1);
- identical output across repeated runs on a larger set (5M events, seed 2);

with local runs capped by `NCPU` and `MEM_GB` and run under `nice`.

**Why.** The fixtures cover rules; the dev set covers dirt at volume; repeat runs catch
nondeterminism (ADR-007) that neither of the other two caught.

---

## Targets (from the track page)

| Run | Data | Limit | Goal |
| --- | --- | --- | --- |
| Dev check | 1M events (seed 1) | 3 min | must match exactly |
| Practice | about 20M events | 2x baseline (171 s), never over 5 min | aim under about 60 s |
| Final | about 60M events | 3 min per run, all 3 runs must finish | median is the score |

Score is run time from container start to exit, lower is better.

## Open questions

- **First submission (`42c1d15`) ran past the practice limit** on about 20M events. The likely
  cause is the quadratic assists join on shared-device persons, removed in ADR-009. Current
  timing: about 7 s for 5M events on 4 laptop cores.
- **Next speed candidates:** the `ev` stage (validation and joins, about 2.7 s at 5M), and keeping
  string IDs out of the large sorts.
- **Docker image not yet built locally** with the added SciPy dependency.
