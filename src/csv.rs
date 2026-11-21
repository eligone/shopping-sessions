//! The rules.md CSV dialect, zero-copy.
//!
//! Only `\n` ends a line; `\r` is an ordinary byte. Every comma separates
//! fields. A field is either bare (no `"`) or wrapped in one pair of quotes
//! with no quote inside. Quotes are syntax; an empty value is null. A line with
//! the wrong field count or a stray quote is malformed, and so is an empty line.

/// Split `line` into exactly `N` fields. Returns false if the line is malformed.
/// `out[i]` is `None` for an empty (null) field.
///
/// Bitmask version (ADR-018): a line of up to 128 bytes becomes two 128-bit masks,
/// one bit per byte, marking commas and quotes. Field count, field boundaries and
/// the quote rule are then bit arithmetic. Longer lines use `fields_scalar`.
#[inline]
pub fn fields<'a, const N: usize>(line: &'a [u8], out: &mut [Option<&'a [u8]>; N]) -> bool {
    let len = line.len();
    if len == 0 {
        return false;
    }
    if len > 128 {
        return fields_scalar(line, out);
    }
    let mut buf = [0u8; 128];
    buf[..len].copy_from_slice(line);
    let (cm, qm) = masks(&buf);
    if cm.count_ones() as usize != N - 1 {
        return false;
    }
    let mut rest = cm;
    let mut s = 0usize;
    for (k, slot) in out.iter_mut().enumerate() {
        let e = if k + 1 < N {
            let p = rest.trailing_zeros() as usize;
            rest &= rest - 1;
            p
        } else {
            len
        };
        let fq = qm & range(s, e);
        let v = if fq == 0 {
            &line[s..e]
        } else {
            if e - s < 2 || fq != ((1u128 << s) | (1u128 << (e - 1))) {
                return false;
            }
            &line[s + 1..e - 1]
        };
        *slot = if v.is_empty() { None } else { Some(v) };
        s = e + 1;
    }
    true
}

/// Comma and quote masks of a 128-byte block: bit i set if byte i matches.
/// Fixed-size loops over 64 bytes, which the compiler turns into SIMD compares.
#[inline]
fn masks(buf: &[u8; 128]) -> (u128, u128) {
    let (mut c0, mut q0, mut c1, mut q1) = (0u64, 0u64, 0u64, 0u64);
    for i in 0..64 {
        c0 |= ((buf[i] == b',') as u64) << i;
        q0 |= ((buf[i] == b'"') as u64) << i;
    }
    for i in 0..64 {
        c1 |= ((buf[64 + i] == b',') as u64) << i;
        q1 |= ((buf[64 + i] == b'"') as u64) << i;
    }
    (c0 as u128 | (c1 as u128) << 64, q0 as u128 | (q1 as u128) << 64)
}

/// Bits s..e (e exclusive) set, for 0 <= s <= e <= 128.
#[inline]
fn range(s: usize, e: usize) -> u128 {
    let hi = if e >= 128 { u128::MAX } else { (1u128 << e) - 1 };
    let lo = if s >= 128 { u128::MAX } else { (1u128 << s) - 1 };
    hi & !lo
}

/// Reference implementation, one byte at a time (the original parser). Kept for
/// lines over 128 bytes and as the oracle for the fuzz test.
#[inline]
pub fn fields_scalar<'a, const N: usize>(line: &'a [u8], out: &mut [Option<&'a [u8]>; N]) -> bool {
    if line.is_empty() {
        return false;
    }
    let mut k = 0usize;
    let mut start = 0usize;
    let n = line.len();
    loop {
        let end = match memchr_comma(&line[start..]) {
            Some(p) => start + p,
            None => n,
        };
        if k == N {
            return false;
        }
        let seg = &line[start..end];
        let v = match memchr_quote(seg) {
            None => seg,
            Some(_) => {
                let l = seg.len();
                if l < 2 || seg[0] != b'"' || seg[l - 1] != b'"' || memchr_quote(&seg[1..l - 1]).is_some() {
                    return false;
                }
                &seg[1..l - 1]
            }
        };
        out[k] = if v.is_empty() { None } else { Some(v) };
        k += 1;
        if end == n {
            break;
        }
        start = end + 1;
    }
    k == N
}

#[inline]
fn memchr_comma(s: &[u8]) -> Option<usize> {
    s.iter().position(|&c| c == b',')
}

#[inline]
fn memchr_quote(s: &[u8]) -> Option<usize> {
    s.iter().position(|&c| c == b'"')
}

/// Iterate the lines of a chunk (split on `\n` only). A trailing `\n` does not
/// produce an extra empty line; empty lines in the middle are yielded and then
/// rejected by `fields`.
#[inline]
pub fn lines(chunk: &[u8]) -> impl Iterator<Item = &[u8]> {
    let body = match chunk.last() {
        Some(b'\n') => &chunk[..chunk.len() - 1],
        _ => chunk,
    };
    let empty = body.is_empty() && chunk.is_empty();
    body.split(|&c| c == b'\n').filter(move |_| !empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f2(s: &str) -> Option<[Option<String>; 2]> {
        let mut o = [None; 2];
        if fields::<2>(s.as_bytes(), &mut o) {
            Some(o.map(|x| x.map(|v| String::from_utf8(v.to_vec()).unwrap())))
        } else {
            None
        }
    }

    #[test]
    fn dialect() {
        let s = |x: &str| Some(x.to_string());
        assert_eq!(f2("u1,u2"), Some([s("u1"), s("u2")]));
        assert_eq!(f2("\"u4\",\"u1\""), Some([s("u4"), s("u1")]));
        assert_eq!(f2("u1,"), Some([s("u1"), None]));
        assert_eq!(f2(",u3"), Some([None, s("u3")]));
        assert_eq!(f2("\"\",x"), Some([None, s("x")]));
        assert_eq!(f2(" u1,u1 "), Some([s(" u1"), s("u1 ")]));
        assert_eq!(f2("\" u1\",a\r"), Some([s(" u1"), s("a\r")]));
        assert_eq!(f2("\"A\tB\",user_id_a"), Some([s("A\tB"), s("user_id_a")]));
        assert_eq!(f2("u1,u2,u3"), None);
        assert_eq!(f2("u1"), None);
        assert_eq!(f2(""), None);
        assert_eq!(f2("u1,u5\""), None);
        assert_eq!(f2("\"u1,u5\""), None);
        assert_eq!(f2("\"u1\"x,u5"), None);
        assert_eq!(f2("\",u5"), None);
    }

    /// Differential fuzz test (ADR-018): the bitmask parser must agree with the
    /// scalar reference on random lines built from the bytes that matter.
    #[test]
    fn fuzz_fields_matches_scalar() {
        let alphabet: &[u8] = b",,,\"\"a b\r\tx\n1Z\xc3\xa9";
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        fn check<const N: usize>(line: &[u8]) {
            let (mut a, mut b) = ([None; N], [None; N]);
            let ra = fields::<N>(line, &mut a);
            let rb = fields_scalar::<N>(line, &mut b);
            assert_eq!(ra, rb, "result for {:?}", String::from_utf8_lossy(line));
            if ra {
                assert_eq!(a, b, "fields for {:?}", String::from_utf8_lossy(line));
            }
        }
        let mut line = Vec::new();
        for i in 0..1_000_000u32 {
            line.clear();
            // Mostly short lines, sometimes long ones to cross 64 and 128 bytes.
            let len = match i % 10 {
                0 => (next() % 200) as usize,
                1 => 60 + (next() % 10) as usize,
                2 => 124 + (next() % 8) as usize,
                _ => (next() % 40) as usize,
            };
            for _ in 0..len {
                line.push(alphabet[(next() % alphabet.len() as u64) as usize]);
            }
            check::<2>(&line);
            check::<4>(&line);
            check::<7>(&line);
        }
        // Realistic lines too: valid shapes with random quoting.
        for _ in 0..200_000 {
            line.clear();
            for k in 0..7 {
                if k > 0 {
                    line.push(b',');
                }
                let q = next() % 4 == 0;
                if q {
                    line.push(b'"');
                }
                for _ in 0..(next() % 12) {
                    line.push(b"abc019 Z:-T"[(next() % 11) as usize]);
                }
                if q {
                    line.push(b'"');
                }
            }
            check::<7>(&line);
        }
    }

    #[test]
    fn line_split() {
        let v: Vec<&[u8]> = lines(b"a\nb\r\n\nc").collect();
        assert_eq!(v, vec![&b"a"[..], b"b\r", b"", b"c"]);
        let v: Vec<&[u8]> = lines(b"a\n").collect();
        assert_eq!(v, vec![&b"a"[..]]);
        assert_eq!(lines(b"").count(), 0);
    }
}
