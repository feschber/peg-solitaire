//! A dense, fixed-size concurrent bitset over `Board`'s 33-bit compressed key space
//! (`Board::to_compressed_repr`/`from_compressed_repr`), used as a replacement for
//! sort+dedup (and, via [`DenseKeySet::test`], sorted-merge intersection) in the
//! hottest rounds of `calculate_feasible_set`.
//!
//! Not used on `wasm32` (a flat 1 GiB allocation is a non-starter in a browser) or,
//! for now, on Android (untested on a real, potentially memory-constrained device -
//! `solitaire-game` calls `calculate_feasible_set` on startup there). Callers keep
//! using the sort+dedup path unconditionally on both; this module still compiles
//! everywhere - only `feasible.rs`'s call sites are gated.

use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;

use crate::Board;

/// number of bits in `Board::to_compressed_repr`'s output; the key space is `2^KEY_BITS`.
const KEY_BITS: usize = Board::SLOTS;
/// one bit per key, `2^KEY_BITS` bits total -> `2^(KEY_BITS - 6)` `u64` words (1 GiB).
const NUM_WORDS: usize = 1 << (KEY_BITS - 6);
/// words per summary bit: each summary bit says "is this whole 512-word (32 Kbit)
/// span of `words` entirely zero?", so extraction/clear can skip it in O(1).
const BLOCK_WORDS: usize = 512;
const NUM_BLOCKS: usize = NUM_WORDS / BLOCK_WORDS;
/// one bit per block, so the summary itself is `2^18 / 64` = 4096 words (32 KiB).
const SUMMARY_WORDS: usize = NUM_BLOCKS / 64;

fn zeroed_atomic_vec(len: usize) -> Vec<AtomicU64> {
    let zeros: Vec<u64> = vec![0u64; len];
    // SAFETY: `AtomicU64` is documented to have the same size, alignment, and bit
    // validity as `u64` (std::sync::atomic::AtomicU64 docs), so a `Vec<u64>` and a
    // `Vec<AtomicU64>` of the same length have identical layout; this only reinterprets
    // the backing allocation, it doesn't touch it. Using this instead of e.g.
    // `(0..len).map(|_| AtomicU64::new(0)).collect()` lets `vec![0u64; len]` satisfy
    // the allocation from the allocator's zeroed-memory path (typically fresh,
    // already-zero OS pages) instead of writing every one of up to 134M words by hand.
    unsafe { std::mem::transmute::<Vec<u64>, Vec<AtomicU64>>(zeros) }
}

pub(crate) struct DenseKeySet {
    words: Vec<AtomicU64>,
    summary: Vec<AtomicU64>,
}

impl DenseKeySet {
    pub(crate) fn new() -> Self {
        Self {
            words: zeroed_atomic_vec(NUM_WORDS),
            summary: zeroed_atomic_vec(SUMMARY_WORDS),
        }
    }

    #[inline]
    pub(crate) fn set(&self, key: u64) {
        let word_idx = (key >> 6) as usize;
        let bit = (key & 63) as u32;
        self.words[word_idx].fetch_or(1 << bit, Ordering::Relaxed);
        let block = word_idx / BLOCK_WORDS;
        self.summary[block / 64].fetch_or(1 << (block % 64), Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn test(&self, key: u64) -> bool {
        let word_idx = (key >> 6) as usize;
        let bit = (key & 63) as u32;
        (self.words[word_idx].load(Ordering::Relaxed) >> bit) & 1 != 0
    }

    /// counts set keys, skipping whole zero blocks via the summary rather than
    /// scanning all 1 GiB - useful for logging without paying for a full extraction.
    pub(crate) fn count_ones(&self) -> usize {
        self.summary
            .par_iter()
            .enumerate()
            .map(|(sword_idx, sword)| {
                let mut bits = sword.load(Ordering::Relaxed);
                let mut count = 0usize;
                while bits != 0 {
                    let block = sword_idx * 64 + bits.trailing_zeros() as usize;
                    for w in &self.words[block * BLOCK_WORDS..(block + 1) * BLOCK_WORDS] {
                        count += w.load(Ordering::Relaxed).count_ones() as usize;
                    }
                    bits &= bits - 1;
                }
                count
            })
            .sum()
    }

    /// clears every key that was set. Cheaper than a flat 1 GiB clear when occupancy
    /// is low (it always is here - even the biggest round sets ~24M of ~8.6B keys):
    /// the summary tells us exactly which blocks need clearing, so untouched blocks
    /// (the vast majority) are skipped entirely.
    pub(crate) fn clear(&self) {
        self.summary.par_iter().enumerate().for_each(|(sword_idx, sword)| {
            let mut bits = sword.swap(0, Ordering::Relaxed);
            while bits != 0 {
                let block = sword_idx * 64 + bits.trailing_zeros() as usize;
                for w in &self.words[block * BLOCK_WORDS..(block + 1) * BLOCK_WORDS] {
                    w.store(0, Ordering::Relaxed);
                }
                bits &= bits - 1;
            }
        });
    }

    /// extracts every set key as a `Board`, in ascending compressed-key order.
    ///
    /// This is a valid, consistent total order for every use in this codebase (see
    /// the plan's audit of `Board`'s `Ord` usage) - it just isn't `Board`'s own `Ord`.
    /// Skips whole zero blocks via the summary rather than scanning all 1 GiB.
    pub(crate) fn extract_sorted_by_key(&self) -> Vec<Board> {
        let chunks: Vec<Vec<Board>> = self
            .summary
            .par_iter()
            .enumerate()
            .map(|(sword_idx, sword)| {
                let bits = sword.load(Ordering::Relaxed);
                if bits == 0 {
                    return Vec::new();
                }
                // count first so this chunk's `Vec` is allocated exactly once, at
                // its final size: pushing into a `Vec::new()` here would otherwise
                // grow it via repeated reallocation, on top of this already being
                // one of many (one per set summary word) small per-chunk
                // allocations that `par_join` below has to then copy out of again.
                let mut count = 0usize;
                let mut b = bits;
                while b != 0 {
                    let block = sword_idx * 64 + b.trailing_zeros() as usize;
                    for w in &self.words[block * BLOCK_WORDS..(block + 1) * BLOCK_WORDS] {
                        count += w.load(Ordering::Relaxed).count_ones() as usize;
                    }
                    b &= b - 1;
                }
                let mut out = Vec::with_capacity(count);
                let mut b = bits;
                while b != 0 {
                    let block = sword_idx * 64 + b.trailing_zeros() as usize;
                    for (wi, w) in self.words[block * BLOCK_WORDS..(block + 1) * BLOCK_WORDS]
                        .iter()
                        .enumerate()
                    {
                        let mut wbits = w.load(Ordering::Relaxed);
                        while wbits != 0 {
                            let bit = wbits.trailing_zeros();
                            let word_idx = block * BLOCK_WORDS + wi;
                            let key = ((word_idx as u64) << 6) | bit as u64;
                            out.push(Board::from_compressed_repr(key));
                            wbits &= wbits - 1;
                        }
                    }
                    b &= b - 1;
                }
                out
            })
            .collect();
        // parallel copy-out instead of `[Vec<T>]::concat()`, which is sequential -
        // that stood out sharply in a flamegraph of this exact call as a single
        // core doing a large memmove while the other 15 sat idle.
        crate::par::par_join(&chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_test_roundtrip() {
        let set = DenseKeySet::new();
        let keys: Vec<u64> = [0, 1, 63, 64, 65, 1 << 20, (1u64 << 33) - 1, 12345, 12345 + 64]
            .into_iter()
            .collect();
        for &k in &keys {
            set.set(k);
        }
        for &k in &keys {
            assert!(set.test(k), "key {k} should be set");
        }
        assert!(!set.test(2), "key 2 was never set");
        assert!(!set.test(66), "key 66 was never set");
    }

    #[test]
    fn concurrent_set_loses_no_bits() {
        let set = DenseKeySet::new();
        // many threads set overlapping/adjacent keys (including keys that land in
        // the same word, to exercise fetch_or races) - none should be lost.
        let keys_per_thread = 5000;
        let nthreads = 8;
        std::thread::scope(|s| {
            for t in 0..nthreads {
                let set = &set;
                s.spawn(move || {
                    for i in 0..keys_per_thread {
                        // interleave keys across threads so many land in the same
                        // 64-bit word (t + i*nthreads covers a dense contiguous range).
                        set.set((t + i * nthreads) as u64);
                    }
                });
            }
        });
        for t in 0..nthreads {
            for i in 0..keys_per_thread {
                let key = (t + i * nthreads) as u64;
                assert!(set.test(key), "key {key} lost under concurrent set()");
            }
        }
        let extracted = set.extract_sorted_by_key();
        assert_eq!(extracted.len(), nthreads * keys_per_thread);
    }

    #[test]
    fn extract_sorted_by_key_matches_and_is_ordered() {
        let set = DenseKeySet::new();
        // spans multiple blocks and multiple summary words.
        let mut keys: Vec<u64> = vec![0, 5, 63, 64, 512 * 64 - 1, 512 * 64, 1 << 20, 1 << 25, (1u64 << 33) - 1];
        for &k in &keys {
            set.set(k);
        }
        let extracted: Vec<u64> = set
            .extract_sorted_by_key()
            .into_iter()
            .map(|b| b.to_compressed_repr())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(extracted, keys);
        assert!(extracted.is_sorted());
    }

    #[test]
    fn clear_removes_everything() {
        let set = DenseKeySet::new();
        for k in [0u64, 100, 1 << 20, 1 << 26] {
            set.set(k);
        }
        assert!(!set.extract_sorted_by_key().is_empty());
        set.clear();
        assert!(set.extract_sorted_by_key().is_empty());
        for k in [0u64, 100, 1 << 20, 1 << 26] {
            assert!(!set.test(k));
        }
    }

    #[test]
    fn empty_set_extracts_nothing() {
        let set = DenseKeySet::new();
        assert!(set.extract_sorted_by_key().is_empty());
    }
}
