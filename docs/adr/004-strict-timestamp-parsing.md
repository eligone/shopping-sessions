# ADR-004: Strict timestamp parsing as one SQL macro, returning NULL when bad

- **Status:** Accepted
- **Date:** 2026-11-21

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
