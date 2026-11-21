//! The rules.md CSV dialect, zero-copy.
//!
//! Only `\n` ends a line; `\r` is an ordinary byte. Every comma separates
//! fields. A field is either bare (no `"`) or wrapped in one pair of quotes
//! with no quote inside. Quotes are syntax; an empty value is null. A line with
//! the wrong field count or a stray quote is malformed, and so is an empty line.

/// Split `line` into exactly `N` fields. Returns false if the line is malformed.
/// `out[i]` is `None` for an empty (null) field.
#[inline]
pub fn fields<'a, const N: usize>(line: &'a [u8], out: &mut [Option<&'a [u8]>; N]) -> bool {
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

    #[test]
    fn line_split() {
        let v: Vec<&[u8]> = lines(b"a\nb\r\n\nc").collect();
        assert_eq!(v, vec![&b"a"[..], b"b\r", b"", b"c"]);
        let v: Vec<&[u8]> = lines(b"a\n").collect();
        assert_eq!(v, vec![&b"a"[..]]);
        assert_eq!(lines(b"").count(), 0);
    }
}
