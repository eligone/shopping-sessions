//! Timestamp parsing: F1 / F2 / F3 -> epoch milliseconds (rules.md, "Timestamps").
//! Returns None for anything that is not one of the three forms or is an
//! impossible calendar value. Window checks are the caller's job.

pub const MS_PER_DAY: i64 = 86_400_000;

#[inline]
fn d2(b: &[u8], i: usize) -> Option<u32> {
    let (x, y) = (b[i].wrapping_sub(b'0'), b[i + 1].wrapping_sub(b'0'));
    if x < 10 && y < 10 { Some(x as u32 * 10 + y as u32) } else { None }
}

#[inline]
fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[inline]
fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => if is_leap(y) { 29 } else { 28 },
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
#[inline]
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// One month near the window: its exact `"YYYY-MM-"` bytes (as a little-endian
/// u64), the day number of its 1st, and its length (ADR-039).
#[derive(Clone, Copy)]
pub struct Month {
    prefix: u64,
    first_day: i64,
    days: u32,
}

static MONTHS: std::sync::OnceLock<Vec<Month>> = std::sync::OnceLock::new();

/// Year and month containing a day number (inverse of `days_from_civil`).
fn civil_from_days(z: i64) -> (i64, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m)
}

/// The months covering [from_ms, to_ms], for the fast path.
pub fn months_between(from_ms: i64, to_ms: i64) -> Vec<Month> {
    let (mut y, mut m) = civil_from_days(from_ms.div_euclid(MS_PER_DAY));
    let (ey, em) = civil_from_days(to_ms.div_euclid(MS_PER_DAY));
    let mut out = Vec::new();
    while (y, m) <= (ey, em) && (0..=9999).contains(&y) {
        let s = format!("{y:04}-{m:02}-");
        out.push(Month { prefix: u64::from_le_bytes(s.as_bytes()[..8].try_into().unwrap()), first_day: days_from_civil(y, m, 1), days: days_in_month(y, m) });
        if m == 12 {
            y += 1;
            m = 1;
        } else {
            m += 1;
        }
    }
    out
}

/// Enable the fast path for timestamps within about 40 days of the window. Call
/// once, before parsing; until then every timestamp uses the full parser.
pub fn init_window(window_start: i64, window_end: i64) {
    let _ = MONTHS.set(months_between(window_start - 40 * MS_PER_DAY, window_end + 40 * MS_PER_DAY));
}

/// `YYYY-MM-DDTHH:MM:SS` via the month table; `None` if the month isn't in it
/// (the caller then uses the full parser), `Some(None)` if it is but the rest is bad.
#[inline]
fn fast_datetime(b: &[u8], months: &[Month]) -> Option<Option<i64>> {
    let p = u64::from_le_bytes(b[..8].try_into().unwrap());
    let m = months.iter().find(|m| m.prefix == p)?;
    Some((|| {
        if b[10] != b'T' || b[13] != b':' || b[16] != b':' {
            return None;
        }
        let d = d2(b, 8)?;
        let h = d2(b, 11)?;
        let mi = d2(b, 14)?;
        let s = d2(b, 17)?;
        if d == 0 || d > m.days || h > 23 || mi > 59 || s > 59 {
            return None;
        }
        let days = m.first_day + d as i64 - 1;
        Some(((days * 24 + h as i64) * 60 + mi as i64) * 60_000 + s as i64 * 1000)
    })())
}

/// Date part + time of day shared by F1 and F2: `YYYY-MM-DDTHH:MM:SS`.
#[inline]
fn parse_datetime(b: &[u8]) -> Option<i64> {
    if let Some(months) = MONTHS.get() {
        if let Some(r) = fast_datetime(b, months) {
            return r;
        }
    }
    parse_datetime_full(b)
}

/// The full parser: any year, full calendar validation.
#[inline]
fn parse_datetime_full(b: &[u8]) -> Option<i64> {
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let y = d2(b, 0)? as i64 * 100 + d2(b, 2)? as i64;
    let mo = d2(b, 5)?;
    let d = d2(b, 8)?;
    let h = d2(b, 11)?;
    let mi = d2(b, 14)?;
    let s = d2(b, 17)?;
    if !(1..=12).contains(&mo) || d == 0 || d > days_in_month(y, mo) || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    Some(((days * 24 + h as i64) * 60 + mi as i64) * 60_000 + s as i64 * 1000)
}

/// Parse a `ts` field. `None` = bad timestamp.
#[inline]
pub fn parse_ts(b: &[u8]) -> Option<i64> {
    match b.len() {
        20 if b[19] == b'Z' => return parse_datetime(b),
        25 if b[19] == b'+' || b[19] == b'-' => {
            if b[22] != b':' {
                return None;
            }
            let oh = d2(b, 20)?;
            let om = d2(b, 23)?;
            if oh > 23 || om > 59 {
                return None;
            }
            let off = (oh as i64 * 60 + om as i64) * 60_000;
            let t = parse_datetime(b)?;
            return Some(if b[19] == b'+' { t - off } else { t + off });
        }
        0 => return None,
        _ => {}
    }
    // F3: ASCII digits only, leading zeros allowed.
    let mut v: i64 = 0;
    let mut sig = 0usize; // significant digits seen
    for &c in b {
        let d = c.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        if sig > 0 || d != 0 {
            sig += 1;
            if sig > 18 {
                // Too long for any window (and possibly for i64): valid form, out of window.
                // Keep scanning so a later non-digit still makes it malformed; either way it is bad.
                continue;
            }
            v = v * 10 + d as i64;
        }
    }
    if sig > 18 { None } else { Some(v) }
}

/// `ingest_ts` is always valid F1; fall back to the general parser just in case.
#[inline]
pub fn parse_ingest(b: &[u8]) -> i64 {
    parse_ts(b).unwrap_or(i64::MAX)
}

/// Parse the manifest's F1 strings.
pub fn parse_f1(s: &str) -> i64 {
    parse_ts(s.as_bytes()).expect("manifest timestamp must be F1")
}

#[cfg(test)]
mod tests {
    use super::*;
    const F1: i64 = 1_791_071_890_000; // 2026-10-03T23:58:10Z

    #[test]
    fn forms() {
        assert_eq!(parse_ts(b"2026-10-03T23:58:10Z"), Some(F1));
        assert_eq!(parse_ts(b"2026-10-03T19:58:10-04:00"), Some(F1));
        assert_eq!(parse_ts(b"1791071890000"), Some(F1));
        assert_eq!(parse_ts(b"0001791071890000"), Some(F1));
        assert_eq!(parse_ts(b"86400000"), Some(86_400_000));
        assert_eq!(parse_ts(b"2026-10-04T01:30:00+02:00"), parse_ts(b"2026-10-03T23:30:00Z"));
        assert_eq!(parse_ts(b"2026-10-03T10:00:00-00:00"), parse_ts(b"2026-10-03T10:00:00Z"));
        assert!(parse_ts(b"2026-10-03T10:00:00+23:59").is_some());
        assert_eq!(parse_ts(b"0"), Some(0));
        assert_eq!(parse_ts(b"000"), Some(0));
    }

    #[test]
    fn bad() {
        for s in [
            &b""[..], b"2026-10-03 10:00:00", b"2026-10-03T10:00:00z", b"2026-10-03t10:00:00Z",
            b"2026-10-03T10:00:00", b"2026-10-03T10:00:00.000Z", b"2026-13-01T00:00:00Z",
            b"2026-02-29T00:00:00Z", b"2026-02-30T00:00:00Z", b"2026-10-03T24:00:00Z",
            b"2026-10-03T23:59:60Z", b"2026-10-03T10:00:00+24:00", b"2026-10-03T10:00:00+05:60",
            b"2026-10-03T10:00:00+0200", b"1791071890000.0", b"1.79e12", b"-5",
            "١٧٩١٠٧١٨٩٠٠٠٠".as_bytes(), "２026-10-03T10:00:00Z".as_bytes(),
            b"123456789012345678901", b"2026-10-03T10:00:00Z\r",
        ] {
            assert_eq!(parse_ts(s), None, "{:?}", String::from_utf8_lossy(s));
        }
        assert!(parse_ts(b"2028-02-29T00:00:00Z").is_some());
        assert!(parse_ts(b"2000-02-29T00:00:00Z").is_some());
        assert!(parse_ts(b"2100-02-29T00:00:00Z").is_none());
    }

    /// The month-table fast path equals the full parser on random datetimes in and
    /// around random windows, including impossible days, hours, separators (ADR-039).
    #[test]
    fn fuzz_fast_datetime_matches_full() {
        let mut seed: u64 = 0x5851_F42D_4C95_7F2D;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200 {
            // A random 14-day window from 2025 to 2028, like the generator.
            let ws_day = days_from_civil(2025, 1, 1) + (next() % (4 * 365)) as i64;
            let ws = ws_day * MS_PER_DAY;
            let months = months_between(ws - 40 * MS_PER_DAY, ws + 54 * MS_PER_DAY);
            for _ in 0..500 {
                let day = ws_day - 60 + (next() % 140) as i64;
                let (y, m) = civil_from_days(day);
                let mut d = (day - days_from_civil(y, m, 1) + 1) as u32;
                if next() % 8 == 0 {
                    d = 28 + (next() % 5) as u32; // 28..32: tests month lengths and Feb 29
                }
                let (mut h, mut mi, mut s) = ((next() % 24) as u32, (next() % 60) as u32, (next() % 60) as u32);
                match next() % 10 {
                    0 => h = 24,
                    1 => mi = 60,
                    2 => s = 60,
                    _ => {}
                }
                let mut str = format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z").into_bytes();
                if next() % 12 == 0 {
                    let at = [10usize, 13, 16, 8, 9][(next() % 5) as usize];
                    str[at] = b"x T:0"[(next() % 5) as usize];
                }
                let full = parse_datetime_full(&str);
                let fast = fast_datetime(&str, &months).unwrap_or_else(|| parse_datetime_full(&str));
                assert_eq!(fast, full, "{}", String::from_utf8_lossy(&str));
            }
        }
    }
}
