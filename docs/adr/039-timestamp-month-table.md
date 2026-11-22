# ADR-039: Timestamp fast path through a table of the window's months

- **Status:** Accepted (branch `lowlevel`)
- **Date:** 2026-11-22

**Context.** The Intel profile put timestamp parsing at about 13% of CPU (`parse_datetime` 6.9%,
`parse_ts` 4.3%, `days_from_civil` 2.0%). Every F1/F2 timestamp and every `ingest_ts` parsed a 4-digit
year, validated month and leap rules, and ran the calendar formula, though nearly all of them fall
within a few weeks of the window.

**Decision.**
- `ts::init_window(ws, we)`, called once in main, builds `MONTHS`: every month from 40 days before
  the window to 40 days after. Each entry holds the exact `"YYYY-MM-"` bytes as a little-endian u64,
  the day number of the 1st, and the month's length.
- `parse_datetime` loads the timestamp's first 8 bytes as one u64 and compares it with those few
  entries. On a match, the year and month are known valid, so only the separators, day (against the
  stored length) and time are checked, and the day number is `first_day + d - 1`.
- No match → the unchanged full parser (`parse_datetime_full`). Before `init_window` (for the
  manifest), everything uses the full parser.

**Verification.**
- `ts::tests::fuzz_fast_datetime_matches_full`: 200 random 14-day windows (2025–2028) × 500
  datetimes within ±60 days, including days 28–32 (month ends, Feb 29), hour 24, minute 60, second 60
  and corrupted separators. Fast path, falling back to full, equals full.
- Mutation check: ignoring month lengths (`d > 31`) fails the test.
- Full check on the Intel VM passes (17/17, fixtures, dev answer, seeds 2–10).

**Measurement (Intel VM, 20M, 5 alternating runs).** Median 2.679 s → **2.401 s** (−0.28 s, −10%);
faster in all 5.

**Scored result.** `f03c56f` (ADR-036 to ADR-039 together) scored **2.01 s** against 2.40 s, matching the
Intel VM's prediction.
