//! Direct-index lookup for IDs shaped "prefix + canonical decimal number" (ADR-032).
//!
//! products.csv and campaigns.csv IDs look like `p4837` and `c_306`. Their common
//! prefix is learned from the file itself (nothing hard-coded). An ID of exactly
//! that prefix followed by a canonical decimal number (ASCII digits, no leading
//! zero unless the number is just "0") maps one-to-one to its number, so a plain
//! array indexed by the number answers the lookup exactly, with no hashing. Any
//! other ID, or a number past the table's end, is reported as "not this shape"
//! or "not present" so the caller can fall back to its exact general path.

/// Largest number given a direct slot; past this an ID falls back to the general path.
const MAX_SLOTS: u64 = 1 << 22;
const ABSENT: u16 = u16::MAX;

pub struct NumIndex {
    prefix: Vec<u8>,
    table: Vec<u16>,
}

pub enum Lookup {
    /// The ID has this index's shape: `Some(v)` if present, `None` if not in the file.
    Shaped(Option<u16>),
    /// The ID does not have the shape: use the general lookup.
    Other,
}

/// The number of `rest` if it is a canonical decimal within the slot limit.
#[inline]
fn canonical_number(rest: &[u8]) -> Option<u64> {
    if rest.is_empty() || rest.len() > 7 || (rest[0] == b'0' && rest.len() > 1) {
        return None;
    }
    let mut n: u64 = 0;
    for &c in rest {
        let d = c.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        n = n * 10 + d as u64;
    }
    if n < MAX_SLOTS { Some(n) } else { None }
}

impl NumIndex {
    /// Learn the prefix from the IDs (the non-digit prefix of the first ID that has
    /// a canonical number after it) and fill the table. First row wins on repeats.
    /// Returns the index and the IDs it did not take, for the general path.
    pub fn build<'a>(ids: impl Iterator<Item = (&'a [u8], u16)>) -> (NumIndex, Vec<(&'a [u8], u16)>) {
        let rows: Vec<(&[u8], u16)> = ids.collect();
        let prefix: Vec<u8> = rows
            .iter()
            .find_map(|(id, _)| {
                let p = id.iter().position(|c| c.is_ascii_digit())?;
                canonical_number(&id[p..]).map(|_| id[..p].to_vec())
            })
            .unwrap_or_default();
        let mut idx = NumIndex { prefix, table: Vec::new() };
        let mut rest = Vec::new();
        for (id, v) in rows {
            match idx.shape(id) {
                Some(n) => {
                    let n = n as usize;
                    if idx.table.len() <= n {
                        idx.table.resize(n + 1, ABSENT);
                    }
                    if idx.table[n] == ABSENT {
                        idx.table[n] = v;
                    }
                }
                None => rest.push((id, v)),
            }
        }
        (idx, rest)
    }

    #[inline]
    fn shape(&self, id: &[u8]) -> Option<u64> {
        if self.prefix.is_empty() || !id.starts_with(&self.prefix) {
            return None;
        }
        canonical_number(&id[self.prefix.len()..])
    }

    #[inline]
    pub fn get(&self, id: &[u8]) -> Lookup {
        match self.shape(id) {
            Some(n) => Lookup::Shaped(self.table.get(n as usize).copied().filter(|&v| v != ABSENT)),
            None => Lookup::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Random catalogs and queries: direct index + fallback must equal a plain
    /// first-wins HashMap lookup by bytes.
    #[test]
    fn fuzz_matches_hashmap() {
        let mut seed: u64 = 0xE703_7ED1_A0B4_28DB;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let make_id = |next: &mut dyn FnMut() -> u64| -> Vec<u8> {
            let prefixes: [&[u8]; 5] = [b"p", b"c_", b"P", b"", b"pp"];
            let mut id = prefixes[(next() % 5) as usize].to_vec();
            match next() % 5 {
                0 => id.extend_from_slice(b"0"),
                1 => id.extend_from_slice(format!("0{}", next() % 100).as_bytes()), // leading zero
                2 => id.extend_from_slice(format!("{}", next() % 100_000_000).as_bytes()), // may be too big
                3 => id.extend_from_slice(format!("{}x", next() % 1000).as_bytes()),
                _ => id.extend_from_slice(format!("{}", 1 + next() % 60_000).as_bytes()),
            }
            id
        };
        for round in 0..30 {
            let catalog: Vec<Vec<u8>> = (0..(50 + round * 200)).map(|_| make_id(&mut next)).collect();
            let mut reference: HashMap<Vec<u8>, u16> = HashMap::new();
            for (i, id) in catalog.iter().enumerate() {
                reference.entry(id.clone()).or_insert(i as u16);
            }
            let (idx, rest) = NumIndex::build(catalog.iter().enumerate().map(|(i, id)| (id.as_slice(), i as u16)));
            let mut fallback: HashMap<Vec<u8>, u16> = HashMap::new();
            for (id, v) in rest {
                fallback.entry(id.to_vec()).or_insert(v);
            }
            for _ in 0..3000 {
                let q = if next() % 2 == 0 { catalog[(next() % catalog.len() as u64) as usize].clone() } else { make_id(&mut next) };
                let got = match idx.get(&q) {
                    Lookup::Shaped(v) => v,
                    Lookup::Other => fallback.get(&q).copied(),
                };
                assert_eq!(got, reference.get(&q).copied(), "query {:?}", String::from_utf8_lossy(&q));
            }
        }
    }
}
