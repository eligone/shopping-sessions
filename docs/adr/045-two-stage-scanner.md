# ADR-045: Two-stage CSV scanner (separator index, then fixed-shape lines)

**Status:** Accepted (branch `lowlevel`), measured together with ADR-046
**Date:** 2026-11-22

## Context
"Scan lines + fields" cost 0.326 s of the events parse on the VM: a branchy walk over
separator bits, with a quote count, a field-count check and an Option built per separator.

## Decision
`for_each_line` now runs two stages per window of about 16K separators:
1. For each 64-byte block (AVX2 masks, ADR-024): append every separator to a small index
   as (position | newline bit, quotes before it). There are no branches on the data, and the writes are unchecked
   after one `reserve(64)` per block.
2. For each line: the next N entries must be N-1 commas then one newline, or the line is skipped
   up to its newline. A field's quote count is the difference of two entries. The field is bare
   (0 quotes) or wrapped (2 quotes at its two ends), as before.
A missing final newline is handled by a virtual newline at the chunk's end. A line longer than a
window carries over to the next window.

## Why it is exact
The rules are unchanged; the existing differential fuzz test (`fuzz_chunk_scanner_matches_reference`,
against `lines` + `fields_scalar`) now also runs with index windows of 1, 2, 3, 7 and 16 entries,
so lines cross window boundaries.

## Measurements (VM, 20M)
Scan stage alone (parse ablation): 0.326 → 0.273 s. Whole run: 2.057 → 2.022 s on its own (5 runs,
every run faster), and −0.066 s together with ADR-046 (see there).
