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

/// Opts `region` out of transparent huge pages.
///
/// mimalloc calls `madvise(..., MADV_HUGEPAGE)` over the arena this 1 GiB bitmap
/// is served from (confirmed by strace: a single call whose length is exactly
/// 1073741824). Where `/sys/kernel/mm/transparent_hugepage/defrag` is `madvise`
/// or `always` - `madvise` is the default on this distro - that turns every fault
/// in the region into a "try hard" huge-page allocation, which drops into
/// *synchronous* direct compaction when no free 2 MiB block is available:
/// `__alloc_pages_direct_compact` -> `compact_zone` -> `migrate_pages`, physically
/// relocating pages on the fault path.
///
/// That trade is catastrophically bad here. On a machine whose memory had become
/// fragmented, the identical binary doing identical userspace work (2.3 CPU-seconds
/// either way) spent 2.75s of system time instead of 0.44s, taking 0.66s wall
/// instead of 0.30s - a kernel profile attributed ~95% of it to
/// `__do_huge_pmd_anonymous_page` and ~89% to compaction. Meanwhile
/// `compact_fail`/`compact_stall` showed 98% of those attempts failing, and the
/// process ended up with `AnonHugePages: 0 kB`: we paid for the compaction and got
/// no huge pages at all.
///
/// Huge pages would in principle suit this access pattern - the bitmap is probed
/// randomly across 1 GiB - but hardware counters bound that upside at ~4.5% of
/// cycles, against a measured downside of several hundred percent. So opt out
/// rather than leave it to how the host happens to be tuned.
#[cfg(target_os = "linux")]
fn disable_transparent_hugepages(region: &[AtomicU64]) {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return;
    }
    let start = region.as_ptr() as usize;
    let end = start + std::mem::size_of_val(region);
    // madvise requires a page-aligned start; round inward so the advice only ever
    // covers pages belonging to this allocation (mimalloc hands back 2 MiB-aligned
    // memory here, so in practice nothing is trimmed).
    let start = start.next_multiple_of(page_size as usize);
    if start >= end {
        return;
    }
    // SAFETY: [start, end) lies within the live `region` allocation. MADV_NOHUGEPAGE
    // only tells the kernel how to back the range with page tables - it does not
    // read, write, move or unmap it - so it can invalidate neither the reference nor
    // the zeroed contents. The result is ignored: this is advisory, and a kernel
    // without THP support rejecting it is fine.
    unsafe {
        libc::madvise(
            start as *mut libc::c_void,
            end - start,
            libc::MADV_NOHUGEPAGE,
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn disable_transparent_hugepages(_region: &[AtomicU64]) {}

pub(crate) struct DenseKeySet {
    words: Vec<AtomicU64>,
    summary: Vec<AtomicU64>,
}

impl DenseKeySet {
    pub(crate) fn new() -> Self {
        let words = zeroed_atomic_vec(NUM_WORDS);
        // only `words` is worth advising; `summary` is 32 KiB, far below the 2 MiB
        // a transparent huge page would need.
        disable_transparent_hugepages(&words);
        Self {
            words,
            summary: zeroed_atomic_vec(SUMMARY_WORDS),
        }
    }

    /// Marks `key` present. Safe to call from many threads at once, but must never
    /// overlap [`DenseKeySet::clear`] - callers always join the filling parallel
    /// region first (see `feasible.rs`, where `clear()` is its own joined region
    /// that runs before generation starts).
    ///
    /// INVARIANT, relied on by [`Self::clear`] and [`Self::extract_sorted_by_key`],
    /// both of which skip whole zero summary
    /// words: if a bit in `words` is set then that bit's block is marked in
    /// `summary`. Every call reaches the summary code below, so each setter either
    /// marks the block itself or observes it already marked - which, by induction,
    /// means some call did mark it. Readers all run after the generating rayon
    /// region has joined, and that join supplies the happens-before edge making
    /// these `Relaxed` writes visible; that is what the surrounding code already
    /// relied on.
    #[inline]
    pub(crate) fn set(&self, key: u64) {
        let word_idx = (key >> 6) as usize;
        let mask = 1u64 << (key & 63);
        // Written unconditionally, on purpose. Guarding this with a `load` first
        // looks attractive (~85% of the raw moves per round are duplicates of a
        // key already set, so most of these RMWs are redundant) but measured
        // *worse* than just writing: the line has to be pulled from DRAM either
        // way, so the load doesn't avoid the miss, it just adds a second
        // dependent access in front of it - and on the round that runs against a
        // freshly allocated bitmap it also faults each page in twice, once
        // read-only against the shared zero page and again for the write. Tried
        // both ways; unconditional store won by ~34ms on a ~330ms run.
        //
        // Should a distinct-key count ever be wanted here, resist getting it by
        // returning whether the key was new. `fetch_or` does hand back the previous
        // word, and exactly one racer can observe a given bit's 0 -> 1 transition,
        // so such a count would be exact and looks free - but *consuming* the return
        // value makes LLVM emit a `lock cmpxchg` retry loop instead of a single
        // `lock or`, which measured ~36ms worse over the ~34M calls these rounds
        // make. Measured, then reverted.
        self.words[word_idx].fetch_or(mask, Ordering::Relaxed);

        let block = word_idx / BLOCK_WORDS;
        let sword = &self.summary[block / 64];
        let smask = 1u64 << (block % 64);
        // The summary is the opposite case, and this is where the win is. It is
        // only 4096 words (~512 cache lines) but every thread writes it, so an
        // unconditional `fetch_or` here is a stream of contended cross-core
        // ownership transfers on a handful of lines. It is also L1-resident and
        // every block that will ever be non-empty gets marked within its first
        // few keys, so this load almost always hits in cache, finds the bit
        // already set, and skips the RMW entirely - keeping the line Shared
        // instead of bouncing it. Worth ~24-29ms per bitset round here.
        if sword.load(Ordering::Relaxed) & smask == 0 {
            sword.fetch_or(smask, Ordering::Relaxed);
        }
    }

    /// Starts fetching the cache line [`Self::set`] would touch for `key`, without
    /// waiting for it.
    ///
    /// This is the counterpart to the "don't guard the `fetch_or` with a load"
    /// comment above, and the reason that guard lost while this wins. `set` scatters
    /// randomly over a 1 GiB bitmap at ~0.3% occupancy, so essentially every call
    /// misses to DRAM - but the cost that dominates is not the miss, it is that a
    /// `lock`ed RMW is a full barrier: it drains the store buffer, so consecutive
    /// independent misses cannot overlap in the core's line-fill buffers and each
    /// one pays the full latency in series. A guard load does not fix that (it is
    /// dependent, and touches the same line); issuing the fetch some distance ahead
    /// of the RMW does, restoring the memory-level parallelism the `lock` prefix
    /// otherwise destroys. `generate_into_bitset` is the only caller and pipelines
    /// it against its own key generation - see the ring buffer there.
    ///
    /// `_MM_HINT_ET0` would be the natural hint (write intent -> line arrives
    /// Modified, so the RMW needs no second round trip for ownership), but LLVM's
    /// x86 backend lowers it to a plain `prefetcht0` anyway. That is close enough
    /// in practice here: nothing else is writing these lines concurrently in the
    /// common case, so they arrive Exclusive and the RMW can upgrade locally.
    #[inline]
    pub(crate) fn prefetch_for_set(&self, key: u64) {
        #[cfg(target_arch = "x86_64")]
        {
            // `key` is a `Board::to_compressed_repr` output, so `key < 2^KEY_BITS`
            // and `word_idx < NUM_WORDS` - the same bound `set`/`test` index under.
            let word_idx = (key >> 6) as usize;
            debug_assert!(word_idx < NUM_WORDS);
            // SAFETY: `word_idx` is in bounds per the above, so the pointer is within
            // the `words` allocation. A prefetch is a hint with no architectural
            // effect - it neither reads nor writes - so it cannot race with the
            // concurrent `set` calls happening around it.
            unsafe {
                core::arch::x86_64::_mm_prefetch::<{ core::arch::x86_64::_MM_HINT_T0 }>(
                    self.words.as_ptr().add(word_idx).cast::<i8>(),
                );
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = key;
    }

    #[inline]
    pub(crate) fn test(&self, key: u64) -> bool {
        let word_idx = (key >> 6) as usize;
        let bit = (key & 63) as u32;
        (self.words[word_idx].load(Ordering::Relaxed) >> bit) & 1 != 0
    }

    /// Reinterprets an exclusively-borrowed atomic slice as plain `u64`s.
    ///
    /// Atomics are only needed while [`Self::set`] is running concurrently. The
    /// bulk operations below run between joined parallel regions, and `&mut`
    /// proves it - so they can use plain loads and stores. That is not just
    /// cosmetic: even a `Relaxed` atomic access is opaque to LLVM's loop
    /// optimizations, so a loop of `store(0, Relaxed)` stays one `mov` per word
    /// and a loop of `load(Relaxed).count_ones()` will not vectorize, whereas the
    /// plain-`u64` equivalents lower to `memset` and to vectorized popcounts.
    fn as_plain(v: &mut [AtomicU64]) -> &mut [u64] {
        // SAFETY: `AtomicU64` is documented to have the same size, alignment and
        // bit validity as `u64`, so the reinterpretation is layout-valid; and the
        // `&mut` borrow proves no other thread can be accessing this memory, so
        // non-atomic access here cannot race. Same guarantee `zeroed_atomic_vec`
        // relies on, in the other direction.
        unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr().cast::<u64>(), v.len()) }
    }

    /// clears every key that was set. Cheaper than a flat 1 GiB clear when occupancy
    /// is low (it always is here - even the biggest round sets ~24M of ~8.6B keys):
    /// the summary tells us exactly which blocks need clearing, so untouched blocks
    /// (the vast majority) are skipped entirely.
    pub(crate) fn clear(&mut self) {
        let Self { words, summary } = self;
        let words = Self::as_plain(words);
        let summary = Self::as_plain(summary);
        // One chunk per summary word (64 blocks). That keeps the summary's
        // skip-untouched-blocks property - the whole point of the index - while
        // still handing rayon disjoint `&mut` slices to work on.
        words
            .par_chunks_mut(BLOCK_WORDS * 64)
            .zip(summary.par_iter_mut())
            .for_each(|(chunk, sword)| {
                let mut bits = std::mem::replace(sword, 0);
                while bits != 0 {
                    let start = bits.trailing_zeros() as usize * BLOCK_WORDS;
                    chunk[start..start + BLOCK_WORDS].fill(0);
                    bits &= bits - 1;
                }
            });
    }

    /// extracts every set key as a `Board`, in ascending compressed-key order.
    ///
    /// This is a valid, consistent total order for every use in this codebase (see
    /// the plan's audit of `Board`'s `Ord` usage) - it just isn't `Board`'s own `Ord`.
    /// Skips whole zero blocks via the summary rather than scanning all 1 GiB.
    pub(crate) fn extract_sorted_by_key(&mut self) -> Vec<Board> {
        // `&mut` lets this read plain `u64`s rather than `Relaxed` atomics; see
        // `as_plain`. It matters most for the counting pass below, which is a
        // popcount over every word of every touched block - vectorizable as plain
        // loads, not as atomic ones.
        let Self { words, summary } = self;
        let words: &[u64] = Self::as_plain(words);
        let summary: &[u64] = Self::as_plain(summary);
        let chunks: Vec<Vec<Board>> = summary
            .par_iter()
            .enumerate()
            .map(|(sword_idx, &bits)| {
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
                    for w in &words[block * BLOCK_WORDS..(block + 1) * BLOCK_WORDS] {
                        count += w.count_ones() as usize;
                    }
                    b &= b - 1;
                }
                let mut out = Vec::with_capacity(count);
                let mut b = bits;
                while b != 0 {
                    let block = sword_idx * 64 + b.trailing_zeros() as usize;
                    for (wi, w) in words[block * BLOCK_WORDS..(block + 1) * BLOCK_WORDS]
                        .iter()
                        .enumerate()
                    {
                        let mut wbits = *w;
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
        let mut set = DenseKeySet::new();
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

    /// keys per block, i.e. how far apart two keys must be to land in different
    /// blocks (and therefore need different summary bits).
    const KEYS_PER_BLOCK: u64 = (BLOCK_WORDS * 64) as u64;

    #[test]
    fn concurrent_set_spanning_many_blocks_loses_no_bits() {
        // The dense variant above keeps every key inside ~2 blocks of a single
        // summary word, so it barely exercises `set()`'s word-bit/summary-bit
        // invariant. Spread the keys over many blocks AND many summary words, so
        // that a dropped summary update makes whole runs of keys disappear from
        // extraction, while still having several threads collide within each word.
        let mut set = DenseKeySet::new();
        let nthreads = 8;
        let blocks_per_thread = 400usize; // 3200 blocks => spans 50 summary words
        std::thread::scope(|s| {
            for t in 0..nthreads {
                let set = &set;
                s.spawn(move || {
                    for b in 0..blocks_per_thread {
                        let block = t + b * nthreads;
                        let base = block as u64 * KEYS_PER_BLOCK;
                        // a few keys in the block, two of them in the same word so
                        // concurrent fetch_or on one word is still covered
                        for off in [0u64, 1, 63, 64, KEYS_PER_BLOCK - 1] {
                            set.set(base + off);
                        }
                    }
                });
            }
        });
        let mut expected = Vec::new();
        for t in 0..nthreads {
            for b in 0..blocks_per_thread {
                let base = (t + b * nthreads) as u64 * KEYS_PER_BLOCK;
                for off in [0u64, 1, 63, 64, KEYS_PER_BLOCK - 1] {
                    expected.push(base + off);
                }
            }
        }
        expected.sort_unstable();
        let extracted: Vec<u64> = set
            .extract_sorted_by_key()
            .into_iter()
            .map(|b| b.to_compressed_repr())
            .collect();
        // extraction is summary-driven, so this fails if any summary bit was lost
        assert_eq!(extracted, expected);
    }

    #[test]
    fn word_bit_set_implies_summary_bit_set() {
        // Deterministic guard on the invariant that `set()`'s conditional summary
        // update depends on, without relying on a race being hit. Covers both the
        // first write to a block and a repeat write (which takes the branch that
        // skips the summary RMW because the bit is already set).
        let set = DenseKeySet::new();
        let summary_bit_set = |key: u64| {
            let block = (key >> 6) as usize / BLOCK_WORDS;
            set.summary[block / 64].load(Ordering::Relaxed) & (1u64 << (block % 64)) != 0
        };
        for key in [
            0u64,
            63,
            64,
            KEYS_PER_BLOCK - 1,
            KEYS_PER_BLOCK,
            KEYS_PER_BLOCK * 64, // first block of the second summary word
            KEYS_PER_BLOCK * 12345,
            (1u64 << KEY_BITS) - 1,
        ] {
            set.set(key);
            assert!(set.test(key), "word bit missing for {key}");
            assert!(summary_bit_set(key), "summary bit missing for {key}");
            // repeat set() takes the early return; the invariant must still hold
            set.set(key);
            assert!(set.test(key), "word bit lost on repeat set of {key}");
            assert!(summary_bit_set(key), "summary bit lost on repeat set of {key}");
        }
    }

    #[test]
    fn extract_sorted_by_key_matches_and_is_ordered() {
        let mut set = DenseKeySet::new();
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
        let mut set = DenseKeySet::new();
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
        let mut set = DenseKeySet::new();
        assert!(set.extract_sorted_by_key().is_empty());
    }
}
