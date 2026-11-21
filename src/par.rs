//! Small parallel helpers shared by the stages.

use rayon::prelude::*;

/// Concatenate per-task vectors into one vector of exactly the right size,
/// copying the pieces into their own disjoint slices in parallel (ADR-016).
/// The serial alternative (`extend_from_slice` in a loop) uses one thread
/// while the rest wait.
pub fn par_concat<T: Copy + Send + Sync>(parts: Vec<Vec<T>>) -> Vec<T> {
    let total: usize = parts.iter().map(|p| p.len()).sum();
    let mut out: Vec<T> = Vec::with_capacity(total);
    {
        let mut rest = &mut out.spare_capacity_mut()[..total];
        let mut slots = Vec::with_capacity(parts.len());
        for p in &parts {
            let (head, tail) = rest.split_at_mut(p.len());
            slots.push(head);
            rest = tail;
        }
        slots.into_par_iter().zip(parts.par_iter()).for_each(|(dst, src)| {
            for (d, s) in dst.iter_mut().zip(src) {
                d.write(*s);
            }
        });
    }
    // SAFETY: the slots cover exactly the first `total` elements, and each was written once.
    unsafe { out.set_len(total) };
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn concat_keeps_order_and_items() {
        let parts: Vec<Vec<u32>> = vec![vec![1, 2], vec![], vec![3], vec![4, 5, 6], vec![]];
        assert_eq!(super::par_concat(parts), vec![1, 2, 3, 4, 5, 6]);
        assert!(super::par_concat::<u32>(vec![]).is_empty());
        let big: Vec<Vec<u64>> = (0..100).map(|i| (0..i).collect()).collect();
        let want: Vec<u64> = big.iter().flatten().copied().collect();
        assert_eq!(super::par_concat(big), want);
    }
}
