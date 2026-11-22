# ADR-047: Lock-free table for long hex IDs

**Status:** Accepted (branch `lowlevel`)
**Date:** 2026-11-22

## Context
About 1 in 8 user and click IDs have the generator's long shape: a letter plus 32 lowercase hex
digits. They went through the sharded interner (ADR-029): a 33-byte hash, a mutex, a hash-map probe, and a
byte compare in the arena, which costs several cache misses per ID. `pack_escaped` was 6.8% of all CPU.
(Making the interner's inner map use a cheaper hasher gained nothing: 2.012 → 2.007 s, reverted.)

## Decision
An ID of exactly that shape is stored as (letter, 128-bit hex value) in a slot of one fixed
open-addressing table. Its key is `ESC | HEXK | slot`, with bit 62 set. Interner keys never use
bit 62. There is no lock and no arena, and the lookup is one probe. The table is sized from the
input (total CSV bytes / 1024 expected IDs, capacity 2×, rounded up to a power of two: 96 MB of
lazily zeroed memory at 20M) and created before parsing. Every other escaped ID still goes through
the interner.

## Why it is exact
Along an ID's probe sequence, slots only change empty → claimed → ready, or empty → sealed. The
first slot that does not hold another ID decides the ID's key, and one compare-and-swap picks a
single winner:
- **claimed:** the ID lives there. Other threads wait until it is ready, then find it.
- **sealed:** this happens once the table is half full. The ID will never be in the table, and
  every thread sends it to the interner.

So each ID gets exactly one key. Hex keys compare by (letter, hi, lo), which equals byte order for
lowercase hex, and against other kinds of keys by their decoded 33 bytes.

**Tests:**
- `hex_table_concurrent_and_sealing_is_exact`: 8 threads race on 4,000 IDs in tables small enough to
  seal. Every thread must agree, and keys must be distinct and round-trip.
- `hex_keys_roundtrip_and_order`: byte round-trip, and `cmp_ids` equal to byte order against short
  IDs, interned IDs (upper case, 34 characters) and other hex IDs.

## Measurements (AWS m5.2xlarge, 20M)
| Binary | Median (5 runs) | Repeat (5 runs) |
|---|---|---|
| ADR-046 (`abbfef4`) | 2.029 s | 2.009 s |
| ADR-047 | 1.969 s | 1.931 s |
Faster in every run. `scripts/check.sh full` (with the calendar-edge seeds): all checks passed.
