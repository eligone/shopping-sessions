# ADR-014: Parallel `pread` read mode, selectable against mmap

- **Status:** Accepted on branch `rust` (default there); `main` keeps mmap + `WILLNEED`
  ([ADR-013](013-readahead-hints.md)) until scores decide.
- **Date:** 2026-11-21

**Context.** [ADR-013](013-readahead-hints.md) kept mmap and added read-ahead hints. The
alternative was to read the files ourselves so that I/O depth and page faults are under our
control. Which wins depends on whether the scorer's page cache is cold or warm, which isn't
known.

**Decision.** `src/input.rs` has one `Source` type with two modes, chosen by `SS_READ`:
- `pread` (default on this branch): about 4 MB byte-range tasks; each reads its range with
  `pread` into a reused per-thread buffer, so all threads have large reads in flight and there
  are no page faults on file data.
- `mmap`: the ADR-013 behaviour.

Both use one ownership rule: **a task owns the lines that start in its byte range.** It reads
one byte before its range to tell whether its first line starts exactly at the boundary, skips
through the first `\n` (the previous task's partial line, or the file's header), and reads past
its end in 64 KB steps until it finishes its last line.

**Verification.** A new unit test (`input::tests::tasks_partition_lines`) splits edge-case files
(empty lines, no trailing newline, header-only, `\r\n`) at **every possible chunk size** in both
modes and checks the lines are exactly the file's lines, each once. Also 36/36 fixtures and dev
MATCH in both modes, and 5M output identical to the DuckDB reference.

**Measurements (macOS laptop, 4 threads, 5M events, alternating runs).**

| Run | mmap + `WILLNEED` | pread |
| --- | --- | --- |
| 1st (partly cold: mmap took 29,135 page faults) | 1.09 s | **0.84 s** |
| 2nd (warm) | **0.63 s** | 0.78 s |
| 3rd (warm) | **0.65 s** | 0.70 s |

`pread` is about 25% faster cold and about 10% slower warm, because the kernel copies each file
into our buffers (system time 0.23–0.32 s against 0.13 s).

**Consequences.** Submit both and keep whichever the scorer favours. Possible hybrid if the
scorer turns out cold: keep mmap, but pre-fault the pages from all threads in parallel before
parsing.
