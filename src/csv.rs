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
#[cfg_attr(not(test), allow(dead_code))] // kept as a fuzz-test oracle (ADR-024)
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
#[cfg_attr(not(test), allow(dead_code))] // kept as a fuzz-test oracle (ADR-024)
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
#[cfg_attr(not(test), allow(dead_code))] // kept as a fuzz-test oracle (ADR-024)
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

/// Newline, comma and quote masks of the 64 bytes at `p` (bit i = byte i matches).
/// AVX2 on x86-64 (the scorer: compare 32 bytes, then movemask); portable loops elsewhere.
#[inline(always)]
fn block_masks(b: &[u8; 64]) -> (u64, u64, u64) {
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    unsafe {
        use std::arch::x86_64::*;
        let lo = _mm256_loadu_si256(b.as_ptr() as *const __m256i);
        let hi = _mm256_loadu_si256(b.as_ptr().add(32) as *const __m256i);
        let m = |v: __m256i, c: u8| _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, _mm256_set1_epi8(c as i8))) as u32 as u64;
        (
            m(lo, b'\n') | m(hi, b'\n') << 32,
            m(lo, b',') | m(hi, b',') << 32,
            m(lo, b'"') | m(hi, b'"') << 32,
        )
    }
    #[cfg(not(all(target_arch = "x86_64", target_feature = "avx2")))]
    {
        let (mut n, mut c, mut q) = (0u64, 0u64, 0u64);
        for i in 0..64 {
            n |= ((b[i] == b'\n') as u64) << i;
            c |= ((b[i] == b',') as u64) << i;
            q |= ((b[i] == b'"') as u64) << i;
        }
        (n, c, q)
    }
}

/// Call `f` with the `N` fields of every well-formed line in `chunk`, in order (ADR-024).
///
/// One pass over the chunk in 64-byte blocks: newline/comma/quote bitmasks, then
/// a walk over the separator bits. Same rules as `lines` + `fields`: only `\n`
/// ends a line; a line must have exactly N fields; a field is bare or wrapped in
/// exactly one pair of quotes with none inside; an empty value is null; empty
/// and malformed lines are skipped.
#[inline(always)] // ADR-038: inline into each caller so the per-line callback inlines too
pub fn for_each_line<'a, const N: usize>(chunk: &'a [u8], mut f: impl FnMut(&[Option<&'a [u8]>; N])) {
    let len = chunk.len();
    let mut out: [Option<&'a [u8]>; N] = [None; N];
    let mut ls = 0usize; // line start
    let mut fs = 0usize; // field start
    let mut k = 0usize; // fields completed in this line
    let mut bad = false;
    let mut q = 0u32; // quotes seen in the current field so far
    let mut base = 0usize;
    let mut tail = [0u8; 64];
    while base < len {
        let block: &[u8; 64] = if base + 64 <= len {
            chunk[base..base + 64].try_into().unwrap()
        } else {
            tail.fill(0);
            tail[..len - base].copy_from_slice(&chunk[base..]);
            &tail
        };
        let (nl, cm, qt) = block_masks(block);
        let mut seps = nl | cm;
        let mut from = 0u32; // first bit of the current field within this block
        while seps != 0 {
            let i = seps.trailing_zeros();
            seps &= seps - 1;
            let p = base + i as usize;
            // Quotes in the field's part inside this block: bits from..i.
            let span = if i == 0 { 0 } else { (u64::MAX >> (64 - i)) & (u64::MAX << from) };
            q += (qt & span).count_ones();
            let is_nl = nl >> i & 1 == 1;
            if !bad {
                let v = if q == 0 {
                    Some(&chunk[fs..p])
                } else if q == 2 && p - fs >= 2 && chunk[fs] == b'"' && chunk[p - 1] == b'"' {
                    Some(&chunk[fs + 1..p - 1])
                } else {
                    None
                };
                match v {
                    None => bad = true,
                    Some(v) => {
                        if k < N {
                            out[k] = if v.is_empty() { None } else { Some(v) };
                        }
                        k += 1;
                        if !is_nl && k >= N {
                            bad = true; // too many fields
                        }
                    }
                }
            }
            if is_nl {
                if !bad && p > ls && k == N {
                    f(&out);
                }
                ls = p + 1;
                k = 0;
                bad = false;
            }
            fs = p + 1;
            q = 0;
            from = i + 1;
        }
        // Quotes after the last separator belong to the field that continues.
        if from < 64 {
            q += (qt & (u64::MAX << from)).count_ones();
        }
        base += 64;
    }
    // A last line without a trailing newline.
    if ls < len {
        let p = len;
        if !bad {
            let v = if q == 0 {
                Some(&chunk[fs..p])
            } else if q == 2 && p - fs >= 2 && chunk[fs] == b'"' && chunk[p - 1] == b'"' {
                Some(&chunk[fs + 1..p - 1])
            } else {
                None
            };
            if let Some(v) = v {
                if k < N {
                    out[k] = if v.is_empty() { None } else { Some(v) };
                }
                k += 1;
                if k == N {
                    f(&out);
                }
            }
        }
    }
}

/// Iterate the lines of a chunk (split on `\n` only). A trailing `\n` does not
/// produce an extra empty line; empty lines in the middle are yielded and then
/// rejected by `fields`.
#[cfg_attr(not(test), allow(dead_code))] // used by the fuzz-test oracles and input tests
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

    /// Differential fuzz test (ADR-024): the chunk scanner must emit exactly the
    /// lines and fields that `lines` + `fields_scalar` accept, in the same order.
    #[test]
    fn fuzz_chunk_scanner_matches_reference() {
        let alphabet: &[u8] = b",,,\"\"\n\na b\r\tx1Z";
        let mut seed: u64 = 0xD1B5_4A32_D192_ED03;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        fn check<const N: usize>(chunk: &[u8]) {
            let mut want: Vec<[Option<Vec<u8>>; N]> = Vec::new();
            for line in lines(chunk) {
                let mut o = [None; N];
                if fields_scalar::<N>(line, &mut o) {
                    want.push(o.map(|x| x.map(|v| v.to_vec())));
                }
            }
            let mut got: Vec<[Option<Vec<u8>>; N]> = Vec::new();
            for_each_line::<N>(chunk, |o| got.push(o.map(|x| x.map(|v| v.to_vec()))));
            assert_eq!(got, want, "chunk {:?}", String::from_utf8_lossy(chunk));
        }
        let mut chunk = Vec::new();
        for i in 0..60_000u32 {
            chunk.clear();
            let len = match i % 4 {
                0 => (next() % 8) as usize,
                1 => (next() % 70) as usize,
                2 => 60 + (next() % 140) as usize,
                _ => (next() % 400) as usize,
            };
            for _ in 0..len {
                chunk.push(alphabet[(next() % alphabet.len() as u64) as usize]);
            }
            check::<2>(&chunk);
            check::<4>(&chunk);
            check::<7>(&chunk);
        }
        // Realistic chunks: many well-formed 7-field lines with random quoting, some broken.
        for _ in 0..3_000 {
            chunk.clear();
            for _ in 0..(next() % 30) {
                for k in 0..7 {
                    if k > 0 {
                        chunk.push(b',');
                    }
                    let quoted = next() % 4 == 0;
                    if quoted {
                        chunk.push(b'"');
                    }
                    for _ in 0..(next() % 14) {
                        chunk.push(b"abc019 Z:-T"[(next() % 11) as usize]);
                    }
                    if quoted && next() % 50 != 0 {
                        chunk.push(b'"');
                    }
                }
                if next() % 40 == 0 {
                    chunk.push(b',');
                }
                chunk.push(b'\n');
            }
            if next() % 2 == 0 {
                chunk.pop(); // sometimes no trailing newline
            }
            check::<7>(&chunk);
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
