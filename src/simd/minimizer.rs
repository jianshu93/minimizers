use std::iter::zip;

use crate::{
    simd::{canonical, linearize},
    Captures,
};

use super::{
    canonical::canonical_mapper,
    collect::{collect, collect_and_dedup},
    dedup,
    nthash::{nthash32_par_it, nthash32_scalar_it, nthash_mapper},
    sliding_min::{
        sliding_lr_min_mapper, sliding_lr_min_par_it, sliding_min_mapper, sliding_min_scalar_it,
    },
};
use itertools::Itertools;
use packed_seq::{Seq, S};

/// Returns the minimizer of a window using a naive linear scan.
pub fn minimizer_window_naive<'s>(seq: impl Seq<'s>, k: usize) -> usize {
    nthash32_scalar_it::<false>(seq, k)
        .map(|x| x & 0xffff_0000)
        .position_min()
        .unwrap()
}

/// Returns an iterator over the absolute positions of the minimizers of a sequence.
/// Returns one value for each window of size `w+k-1` in the input. Use
/// `Itertools::dedup()` to obtain the distinct positions of the minimizers.
///
/// Prefer `minimizer_simd_it` that internally used SIMD, or `minimizer_par_it` if it works for you.
pub fn minimizer_scalar_it<'s>(
    seq: impl Seq<'s>,
    k: usize,
    w: usize,
) -> impl ExactSizeIterator<Item = u32> + Captures<&'s ()> {
    let it = nthash32_scalar_it::<false>(seq, k);
    sliding_min_scalar_it::<true>(it, w)
}

/// Returns an iterator over the absolute positions of the minimizers of a sequence.
/// Returns one value for each window of size `w+k-1` in the input. Use
/// `Itertools::dedup()` to obtain the distinct positions of the minimizers.
///
/// This splits the windows of the sequence into chunks of 2^13.
/// Minimizers for each chunk are eagerly computed using 8 parallel streams using SIMD using `minimizers_par_it`.
/// Then returns a linear iterator over the buffer.
/// Once the buffer runs out, the next chunk is computed.
///
/// NOTE: This method is ~4x slower than the minimizer computation itself, and
///       only ~2x faster than the scalar version. Mostly because shuffling memory is slow.
/// TODO: Fix this.
pub fn minimizer_simd_it<'s>(
    seq: impl Seq<'s>,
    k: usize,
    w: usize,
) -> impl ExactSizeIterator<Item = u32> + Captures<&'s ()> {
    linearize::linearize_with_offset(seq, k + w - 1, move |seq| minimizer_par_it(seq, k, w))
}

/// Split the windows of the sequence into 8 chunks of equal length ~len/8.
/// Then return the positions of the minimizers of each of them in parallel using SIMD,
/// and return the remaining few using the second iterator.
// TODO: Take a hash function as argument.
pub fn minimizer_par_it<'s>(
    seq: impl Seq<'s>,
    k: usize,
    w: usize,
) -> (
    impl ExactSizeIterator<Item = S> + Captures<&'s ()>,
    impl ExactSizeIterator<Item = u32> + Captures<&'s ()>,
) {
    let l = k + w - 1;

    let (add_remove, tail) = seq.par_iter_bp_delayed(k + w - 1, k - 1);

    let mut nthash = nthash_mapper::<false>(k, w);
    // let mut alex = alex::alex_mapper(k, w);
    let mut sliding_min = sliding_min_mapper::<true>(w, k, add_remove.size_hint().0);

    let mut head = add_remove.map(move |(a, rk)| {
        let nthash = nthash((a, rk));
        sliding_min(nthash)
        // let alex = alex(a);
        // sliding_min(alex)
    });

    head.by_ref().take(l - 1).for_each(drop);
    let head_len = head.size_hint().0;

    let tail = minimizer_scalar_it(tail, k, w).map(move |p| p + 8 * head_len as u32);
    (head, tail)
}

pub fn minimizers_collect<'s>(seq: impl Seq<'s>, k: usize, w: usize) -> Vec<u32> {
    let (par_head, tail) = minimizer_par_it(seq, k, w);
    collect(par_head, tail)
}

/// Prefer `minimizers_collect_and_dedup`
#[doc(hidden)]
pub fn minimizers_dedup<'s>(seq: impl Seq<'s>, k: usize, w: usize) -> Vec<u32> {
    let (par_head, tail) = minimizer_par_it(seq, k, w);
    let mut positions = collect(par_head, tail);
    dedup(&mut positions);
    positions
}

pub fn minimizers_collect_and_dedup<'s, const SUPER: bool>(
    seq: impl Seq<'s>,
    k: usize,
    w: usize,
    out_vec: &mut Vec<u32>,
) {
    let (par_head, tail) = minimizer_par_it(seq, k, w);
    collect_and_dedup::<SUPER>(par_head, tail, out_vec);
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// TRULY CANONICAL MINIMIZERS BELOW HERE
// The minimizers above can take a canonical hash, but do not correctly break ties.
// Below we fix that.

pub fn canonical_minimizer_scalar_it<'s>(
    seq: impl Seq<'s>,
    k: usize,
    w: usize,
) -> impl ExactSizeIterator<Item = u32> + Captures<&'s ()> {
    // true: canonical
    let kmer_hashes = nthash32_scalar_it::<true>(seq, k);
    // true: leftmost
    let left = sliding_min_scalar_it::<true>(kmer_hashes.clone(), w);
    // false: rightmost
    let right = sliding_min_scalar_it::<false>(kmer_hashes, w);
    // indicators whether each window is canonical
    let canonical = canonical::canonical_scalar_it(seq, k, w);
    zip(canonical, zip(left, right)).map(|(canonical, (left, right))| {
        // Select left or right based on canonical mask.
        if canonical {
            left
        } else {
            right
        }
    })
}

/// Use canonical NtHash, and keep both leftmost and rightmost minima.
pub fn canonical_minimizer_par_it<'s>(
    seq: impl Seq<'s>,
    k: usize,
    w: usize,
) -> (
    impl ExactSizeIterator<Item = S> + Captures<&'s ()>,
    impl ExactSizeIterator<Item = u32> + Captures<&'s ()>,
) {
    let (kmer_hashes, kmer_hashes_tail) = nthash32_par_it::<true>(seq, k, w);
    let lr_min = sliding_lr_min_par_it(kmer_hashes, w);
    let (canonical, canonical_tail) = canonical::canonical_par_it(seq, k, w);
    let head = zip(canonical, lr_min).map(
        #[inline(always)]
        |(canonical, (left, right))| {
            // Select left or right based on canonical mask.
            unsafe { std::mem::transmute::<_, S>(canonical).blend(left, right) }
        },
    );

    let left_tail = sliding_min_scalar_it::<true>(kmer_hashes_tail.clone(), w);
    let right_tail = sliding_min_scalar_it::<false>(kmer_hashes_tail, w);
    let tail = zip(canonical_tail, zip(left_tail, right_tail)).map(
        #[inline(always)]
        |(canonical, (left, right))| {
            // Select left or right based on canonical mask.
            if canonical {
                left
            } else {
                right
            }
        },
    );

    (head, tail)
}

/// Use canonical NtHash, and keep both leftmost and rightmost minima.
pub fn canonical_minimizer_par_it_new<'s>(
    seq: impl Seq<'s>,
    k: usize,
    w: usize,
) -> (
    impl ExactSizeIterator<Item = S> + Captures<&'s ()>,
    impl ExactSizeIterator<Item = u32> + Captures<&'s ()>,
) {
    let l = k + w - 1;

    // FIXME: NtHash takes the return value *before* dropping the given character,
    // while canonical first drops the character.
    let (add_remove, tail) = seq.par_iter_bp_delayed_2(k + w - 1, k - 1, l);

    let mut nthash = nthash_mapper::<true>(k, w);
    let mut canonical = canonical_mapper(k, w);
    let mut sliding_min = sliding_lr_min_mapper(w, add_remove.size_hint().0);

    let mut head = add_remove.map(move |(a, rk, rl)| {
        let nthash = nthash((a, rk));
        let canonical = canonical((a, rl));
        let (lmin, rmin) = sliding_min(nthash);
        unsafe { std::mem::transmute::<_, S>(canonical).blend(lmin, rmin) }
    });

    head.by_ref().take(l - 1).for_each(drop);

    let tail = canonical_minimizer_scalar_it(tail, k, w);
    (head, tail)
}

pub fn canonical_minimizer_collect_and_dedup<'s, const SUPER: bool>(
    seq: impl Seq<'s>,
    k: usize,
    w: usize,
    out_vec: &mut Vec<u32>,
) {
    let (par_head, tail) = canonical_minimizer_par_it(seq, k, w);
    collect_and_dedup::<SUPER>(par_head, tail, out_vec);
}

pub fn canonical_minimizer_collect_and_dedup_new<'s, const SUPER: bool>(
    seq: impl Seq<'s>,
    k: usize,
    w: usize,
    out_vec: &mut Vec<u32>,
) {
    let (par_head, tail) = canonical_minimizer_par_it_new(seq, k, w);
    collect_and_dedup::<SUPER>(par_head, tail, out_vec);
}

#[cfg(test)]
mod test {
    use super::*;
    use packed_seq::{AsciiSeq, AsciiSeqVec, PackedSeqVec, SeqVec};
    use std::{cell::LazyCell, iter::once};

    const ASCII_SEQ: LazyCell<AsciiSeqVec> = LazyCell::new(|| AsciiSeqVec::random(1024 * 1024));
    const PACKED_SEQ: LazyCell<PackedSeqVec> = LazyCell::new(|| PackedSeqVec::random(1024 * 1024));

    #[test]
    fn scalar_ascii() {
        let seq = &*ASCII_SEQ;
        for k in [1, 2, 3, 4, 5, 31, 32, 33, 63, 64, 65] {
            for w in [1, 2, 3, 4, 5, 31, 32, 33, 63, 64, 65] {
                for len in (0..100).chain(once(1024 * 32)) {
                    let seq = seq.slice(0..len);
                    let single = seq.0[0..len]
                        .windows(w + k - 1)
                        .enumerate()
                        .map(|(pos, seq)| {
                            (pos + minimizer_window_naive(AsciiSeq::new(seq, w + k - 1), k)) as u32
                        })
                        .collect::<Vec<_>>();
                    let scalar = minimizer_scalar_it(seq, k, w).collect::<Vec<_>>();
                    assert_eq!(single, scalar, "k={k}, w={w}, len={len}");
                }
            }
        }
    }

    #[ignore = "delayed iteration not yet implemented for ASCII"]
    #[test]
    fn simd_ascii() {
        let seq = &*ASCII_SEQ;
        for k in [1, 2, 3, 4, 5, 31, 32, 33, 63, 64, 65] {
            for w in [1, 2, 3, 4, 5, 31, 32, 33, 63, 64, 65] {
                for len in (0..100).chain(once(1024 * 128)) {
                    let seq = seq.slice(0..len);
                    let scalar = minimizer_scalar_it(seq, k, w).collect::<Vec<_>>();
                    let (par_head, tail) = minimizer_par_it(seq, k, w);
                    let par_head = par_head.collect::<Vec<_>>();
                    let parallel_iter = (0..8)
                        .flat_map(|l| par_head.iter().map(move |x| x.as_array_ref()[l]))
                        .chain(tail)
                        .collect::<Vec<_>>();

                    assert_eq!(scalar, parallel_iter, "k={k}, w={w}, len={len}");
                }
            }
        }
    }

    #[test]
    fn simd_packed() {
        let seq = &*PACKED_SEQ;
        for k in [1, 2, 3, 4, 5, 31, 32, 33, 63, 64, 65] {
            for w in [1, 2, 3, 4, 5, 31, 32, 33, 63, 64, 65] {
                for len in (0..100).chain(once(1024 * 128)) {
                    let seq = seq.slice(0..len);
                    let scalar = minimizer_scalar_it(seq, k, w).collect::<Vec<_>>();
                    let (par_head, tail) = minimizer_par_it(seq, k, w);
                    let par_head = par_head.collect::<Vec<_>>();
                    let parallel_iter = (0..8)
                        .flat_map(|l| par_head.iter().map(move |x| x.as_array_ref()[l]))
                        .chain(tail)
                        .collect::<Vec<_>>();
                    if w == 1 && parallel_iter.len() > 0 {
                        assert_eq!(parallel_iter[0], 0);
                    }
                    assert_eq!(scalar, parallel_iter, "k={k}, w={w}, len={len}");
                }
            }
        }
    }

    #[test]
    fn linearized() {
        let seq = &*PACKED_SEQ;
        for k in [1, 2, 3, 4, 5, 31, 32, 33, 63, 64, 65] {
            for w in [1, 2, 3, 4, 5, 31, 32, 33, 63, 64, 65] {
                for len in (0..100).chain(once(1024 * 128 + 765)) {
                    let seq = seq.slice(0..len);
                    let scalar = minimizer_scalar_it(seq, k, w).collect::<Vec<_>>();
                    let simd = minimizer_simd_it(seq, k, w).collect::<Vec<_>>();
                    assert_eq!(
                        scalar,
                        simd,
                        "k={}, len={} len left {} len right {}",
                        k,
                        len,
                        scalar.len(),
                        simd.len()
                    );
                }
            }
        }
    }

    #[test]
    fn test_sliding_min() {
        let n = 1000;
        let seq = PackedSeqVec::random(n);
        let mut poss = vec![];
        for k in 1..10 {
            for w in 1..10 {
                poss.clear();
                minimizers_collect_and_dedup::<false>(seq.as_slice(), k, w, &mut poss);
                for &x in &poss {
                    assert!(
                        x <= (n - k) as u32,
                        "Index {x} is not in range for n={n}, k={k}, w={w}. Should be in 0..{}\n{poss:?}",
                        n - k
                    );
                }
            }
        }
    }
}
