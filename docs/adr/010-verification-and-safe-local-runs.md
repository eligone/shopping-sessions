# ADR-010: Verify against fixtures, the dev answer and repeat runs; keep runs safe on small machines

- **Status:** Accepted
- **Date:** 2026-11-21

**Decision.** Every change must pass all of:
- all 36 fixtures (`scripts/run_fixtures.py`);
- an exact match with `dev-answer/` on the 1M-event dev set (seed 1);
- identical output across repeated runs on a larger set (5M events, seed 2);

with local runs capped by `NCPU` and `MEM_GB` and run under `nice`.

**Why.** The fixtures cover rules; the dev set covers dirt at volume; repeat runs catch
nondeterminism ([ADR-007](007-range-frames-for-ties.md)) that neither of the other two caught.
