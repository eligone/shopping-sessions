# ADR-019: Mutation testing: plant realistic bugs and check the tests catch them

- **Status:** Accepted
- **Date:** 2026-11-21

**Context.** The test suite (unit tests, 36 fixtures, dev answer, Rust-vs-DuckDB diffs) passing
proves nothing if it could not fail. Optimizations rewrite delicate code, so we need evidence the
suite would notice a wrong answer.

**Decision.** `scripts/mutation_test.py` plants 15 realistic bugs, one at a time, across every
module: off-by-one comparisons at each boundary in rules.md, a reversed dedup order, a reversed
tie-break, a flipped offset sign, calendar and offset validation, the quote rule. For each it
rebuilds, runs the checks cheapest-first, records which check failed first, and restores the file.

**Results (`8ddc5b5`).**

| Caught by | Count | Mutations |
| --- | --- | --- |
| fixtures | 8 | late cutoff, window start, dedup order, event type case, 30-min gap, top_category tie, 7-day credit limit, person naming |
| unit tests | 4 | offset sign, Feb 30, offset +24:00, lone quote |
| survived | 3 | all equivalent (below) |

The 3 survivors cannot change any output:
- **Price 0 accepted by the range check:** 0 is also the parser's "invalid price" sentinel, and
  validation rejects purchases priced 0, so the result is the same.
- **Click ranges touching at one instant not merged:** the union of the ranges is the same, so the
  same sessions are assisted.
- **A click at exactly `window_end` kept:** a click only counts for sessions starting at or after it,
  and every session starts before `window_end`.

**Consequences.** Every output-changing mutation is caught (12/12). Run the script after changing
test coverage or before a risky rewrite. It is slow (about 15 rebuilds), so it is not part of
`scripts/check.sh`.
