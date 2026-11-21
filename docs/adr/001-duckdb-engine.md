# ADR-001: Use DuckDB as the engine

- **Status:** Accepted
- **Date:** 2026-11-21

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
