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

/// Date part + time of day shared by F1 and F2: `YYYY-MM-DDTHH:MM:SS`.
#[inline]
fn parse_datetime(b: &[u8]) -> Option<i64> {
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
}
