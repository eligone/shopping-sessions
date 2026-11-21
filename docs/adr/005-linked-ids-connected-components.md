# ADR-005: Linked IDs via connected components, named by the smallest index

- **Status:** Accepted
- **Date:** 2026-11-21

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
