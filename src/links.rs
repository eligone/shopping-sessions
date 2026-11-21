//! Linked IDs -> persons (rules.md, "Linked IDs").
//! Union-find over every ID that appears in a valid link line; each person is
//! named by the byte-wise smallest of its IDs.

use crate::csv;
use crate::ids::{self, Key, KeyMap};

/// Map from every linked ID to its person's name. IDs not in the map are their own person.
pub fn persons(src: &crate::input::Source) -> KeyMap<Key> {
    let t0 = std::time::Instant::now();
    let edges: Vec<(Key, Key)> = src
        .par_map(|c| {
            let mut v = Vec::new();
            csv::for_each_line::<2>(c, |f| {
                if let (Some(a), Some(b)) = (f[0], f[1]) {
                    v.push((ids::pack(a), ids::pack(b)));
                }
            });
            v
        })
        .into_iter()
        .flatten()
        .collect();

    eprintln!("    links parse    {:.3}s ({} edges)", t0.elapsed().as_secs_f64(), edges.len());
    let t1 = std::time::Instant::now();
    // Intern the linked IDs.
    let mut index: KeyMap<u32> = ids::key_map(edges.len() * 2);
    let mut keys: Vec<Key> = Vec::with_capacity(edges.len() * 2);
    let id = |k: Key, index: &mut KeyMap<u32>, keys: &mut Vec<Key>| -> u32 {
        *index.entry(k).or_insert_with(|| {
            keys.push(k);
            (keys.len() - 1) as u32
        })
    };
    let pairs: Vec<(u32, u32)> = edges.iter().map(|&(a, b)| (id(a, &mut index, &mut keys), id(b, &mut index, &mut keys))).collect();

    // Union-find with path halving and union by size.
    let n = keys.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let mut size: Vec<u32> = vec![1; n];
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            let p = parent[x as usize];
            parent[x as usize] = parent[p as usize];
            x = parent[x as usize];
        }
        x
    }
    for &(a, b) in &pairs {
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            let (big, small) = if size[ra as usize] >= size[rb as usize] { (ra, rb) } else { (rb, ra) };
            parent[small as usize] = big;
            size[big as usize] += size[small as usize];
        }
    }

    // Name each component by its byte-wise smallest ID.
    let mut name: Vec<Key> = vec![0; n];
    let mut has: Vec<bool> = vec![false; n];
    let mut root = vec![0u32; n];
    for i in 0..n {
        let r = find(&mut parent, i as u32) as usize;
        root[i] = r as u32;
        if !has[r] || ids::cmp_ids(keys[i], name[r]).is_lt() {
            name[r] = keys[i];
            has[r] = true;
        }
    }
    let mut out: KeyMap<Key> = ids::key_map(n);
    for i in 0..n {
        out.insert(keys[i], name[root[i] as usize]);
    }
    eprintln!("    links dsu+map  {:.3}s ({} ids)", t1.elapsed().as_secs_f64(), n);
    out
}
