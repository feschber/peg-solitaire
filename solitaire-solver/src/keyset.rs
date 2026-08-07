//! A dense, fixed-size concurrent bitset over `Board`'s compressed key space, used
//! as a replacement for sort+dedup (and, via [`DenseKeySet::test`], sorted-merge
//! intersection) in the hottest rounds of `calculate_feasible_set`.
//!
//! Indexed by a key's *rank within its round*, not by the key itself. Every board
//! in a BFS round has the same peg count `k`, so only the `C(33, k)` patterns of
//! popcount `k` can occur - at most `C(33, 16)`, which is 7.4x fewer than the `2^33`
//! a raw key spans. Ranking collapses the map from 1 GiB to 139 MiB, and shrinks the
//! part of it a round actually touches from ~185 MiB to ~34 MiB, which is what the
//! random probes and the page faults both scale with. Measured on the largest
//! round's real key stream (`examples/rank_bench.rs`), against indexing raw keys:
//! `set` 57% faster, `test` 71% faster, and the summary index shrinks 7x with it.
//! [`LOW_BITS`] covers how the rank is made cheap enough to compute per key.
//!
//! The price is that a key only has a meaningful position *within* one layer:
//! [`DenseKeySet::begin_round`] must declare the peg count, and mixing layers would
//! silently alias distinct boards onto one bit rather than fail. `index` asserts the
//! popcount in debug builds, and `begin_round` clears the map so bits can never
//! outlive the ranking that produced them.
//!
//! Not used on `wasm32` (even 139 MiB is a poor citizen in a browser) or,
//! for now, on Android (untested on a real, potentially memory-constrained device -
//! `solitaire-game` calls `calculate_feasible_set` on startup there). Callers keep
//! using the sort+dedup path unconditionally on both; this module still compiles
//! everywhere - only `feasible.rs`'s call sites are gated.

use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;

use crate::Board;

/// number of bits in `Board::to_compressed_repr`'s output.
const KEY_BITS: usize = Board::SLOTS;

/// Largest number of keys any single round can produce: every board in a BFS round
/// has the same peg count `k`, so its keys are not all `2^KEY_BITS` patterns but
/// only the `C(33, k)` with popcount `k`, and `C(33, 16) = C(33, 17)` is the peak.
///
/// This is what the bitmap is sized for, indexed by [`DenseKeySet::index`] rather
/// than by the raw key - 139 MiB instead of 1 GiB, and a round with fewer pegs uses
/// only a low prefix of it. See the module docs for what that buys.
const MAX_LAYER_KEYS: usize = 1_166_803_110;
/// words covered by one summary word, i.e. one unit of the bulk operations below.
/// Padding the bitmap up to a multiple of this keeps every chunk they take full,
/// so none of them need a partial-chunk case.
const CHUNK_WORDS: usize = BLOCK_WORDS * 64;
/// one bit per rankable key, padded as above -> ~139 MiB.
const NUM_WORDS: usize = MAX_LAYER_KEYS.div_ceil(64).next_multiple_of(CHUNK_WORDS);

/// bits of a key handled by the low half of the two-table rank; the high half gets
/// the remaining `KEY_BITS - LOW_BITS`.
///
/// The split exists to make ranking cheap enough for the hot path. The textbook
/// rank - sum `C(p, i)` over the set bits - is a loop of up to 17 dependent
/// lookups; splitting it turns it into two independent table reads and an add:
///
/// ```text
///   index(key) = high_cum[key >> LOW_BITS] + low_rank[key & LOW_MASK]
/// ```
///
/// 16 balances the two tables against the cache: `low_rank` is `2^16` u16s
/// (128 KiB) and `high_cum` is `2^17` u32s (512 KiB), so both sit in L2 while
/// `high_cum`'s values stay under `C(33,16) < 2^32`.
const LOW_BITS: u32 = 16;
const LOW_MASK: u64 = (1 << LOW_BITS) - 1;
const HIGH_ENTRIES: usize = 1 << (KEY_BITS as u32 - LOW_BITS);

/// The key whose rank within the `pegs` layer is `index` - the inverse of
/// [`DenseKeySet::index`].
///
/// `hint` is a prefix index known not to exceed the answer's, so a caller decoding
/// ascending indices can walk `high_cum` forward instead of searching it per key;
/// pass 0 to start from the bottom. Returns the prefix it settled on, to be fed back
/// as the next `hint`.
fn unindex(
    high_cum: &[u32],
    low_unrank: &[Vec<u16>],
    pegs: usize,
    index: u64,
    hint: usize,
) -> (u64, usize) {
    // The answer's prefix is the *last* one whose cumulative count still fits under
    // `index`. Equal neighbouring counts are prefixes holding no keys of this layer
    // (their popcount cannot be completed to `pegs` by 16 low bits), and taking the
    // last of such a run steps past them onto one that does.
    let mut h = hint;
    while h + 1 < HIGH_ENTRIES && high_cum[h + 1] as u64 <= index {
        h += 1;
    }
    let used = (h as u64).count_ones() as usize;
    debug_assert!(
        used <= pegs,
        "index {index} decodes to a prefix with more pegs than the layer holds"
    );
    let low = low_unrank[pegs - used][(index - high_cum[h] as u64) as usize];
    let key = ((h as u64) << LOW_BITS) | low as u64;
    debug_assert_eq!(key.count_ones() as usize, pegs);
    (key, h)
}

/// `binomial[n][k]` = C(n, k), for the rank tables.
fn binomials() -> [[u64; KEY_BITS + 1]; KEY_BITS + 1] {
    let mut c = [[0u64; KEY_BITS + 1]; KEY_BITS + 1];
    for n in 0..=KEY_BITS {
        c[n][0] = 1;
        for k in 1..=n {
            // c[n - 1][k] is legitimately 0 for k > n - 1; do not clamp the index
            c[n][k] = c[n - 1][k - 1] + c[n - 1][k];
        }
    }
    c
}
/// words per summary bit: each summary bit says "is this whole 64-word (4 Kbit)
/// span of `words` entirely zero?", so extraction/clear can skip it in O(1).
///
/// This trades directly against [`DenseKeySet::set`]'s summary update. Coarser
/// blocks mean a smaller summary, which keeps that update in L1; finer blocks mean
/// [`DenseKeySet::clear`] zeroes less untouched memory around each set bit. Keys
/// are extremely sparse here (a round sets ~2M of 8.6B), so a block is almost
/// always cleared for the sake of a handful of bits, and clear is where the money
/// is. Measured end to end, 15 interleaved reps, internal timer medians - plus
/// per-phase totals from an instrumented build:
///
/// ```text
/// BLOCK_WORDS   summary    clear    generate   total wall
///         512     32 KiB   21.8 ms    88.1 ms     181.4 ms
///          64    256 KiB   14.7 ms    92.9 ms     173.3 ms   <- chosen
///          32    512 KiB   12.1 ms    90.1 ms     179.7 ms
/// ```
///
/// 64 is the turning point: clear keeps getting cheaper below it, but the summary
/// outgrows what `set` can keep hot and the generation step gives back more than
/// the clear saves.
const BLOCK_WORDS: usize = 64;
const NUM_BLOCKS: usize = NUM_WORDS / BLOCK_WORDS;
/// one bit per block, so the summary itself is `2^21 / 64` = 32768 words (256 KiB).
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
    /// `low_rank[l]` = how many 16-bit values below `l` share its popcount.
    /// Independent of the round, so built once.
    low_rank: Vec<u16>,
    /// `low_unrank[j]` = the 16-bit values with popcount `j`, ascending; the inverse
    /// of `low_rank`, needed only by [`DenseKeySet::extract_sorted_by_key`].
    low_unrank: Vec<Vec<u16>>,
    /// `high_cum[h]` = keys below the prefix `h << LOW_BITS` that have this round's
    /// peg count. Depends on that count, so rebuilt by [`DenseKeySet::begin_round`].
    high_cum: Vec<u32>,
    /// peg count shared by every key in the map this round; `index` is only a
    /// bijection within one such layer.
    pegs: usize,
}

impl DenseKeySet {
    pub(crate) fn new() -> Self {
        let words = zeroed_atomic_vec(NUM_WORDS);
        // only `words` is worth advising; `summary` is 256 KiB, still well below
        // the 2 MiB a transparent huge page would need.
        disable_transparent_hugepages(&words);
        // Deliberately NOT prefaulted, and this is worth stating because a profile
        // of the whole process makes it look like it should be: 13.1% of cycles sit
        // under `exc_page_fault`/`handle_mm_fault` and 3.7% in `kernel_init_pages`,
        // spread through the run, which is first touch of this mapping arriving one
        // 4 KiB page at a time (4 KiB because of the advice above).
        //
        // Populating it up front with `MADV_POPULATE_WRITE`, issued from parallel
        // chunks so as not to serialize what the lazy faults do across all workers,
        // measured *worse* by 38 ms (+25.8%, slower in 14 of 14 interleaved reps).
        //
        // Because only a quarter of this mapping is ever touched: peak RSS of the
        // 1 GiB region is 261 MiB, reproducibly. The keys are not spread over the
        // space uniformly - `normalize` returns the minimum of each board's
        // 8-symmetry orbit, and a minimum-of-8 leaves the high bits of the
        // compressed key clear far more often than not, so the keys crowd into the
        // low quarter of the range. Prefaulting therefore commits and zeroes ~760
        // MiB that nothing will ever read, which is the whole of the regression.
        //
        // So the fault cost in the profile is first touch of memory the run
        // genuinely uses, not waste, and the untouched three quarters of the
        // mapping costs nothing but address space. Shrinking the key space - e.g.
        // ranking each round's boards within `C(33, pegs)` instead of `2^33` - is
        // the lever that would actually cut it, at the price of computing the rank.
        let c = binomials();
        // low_rank / low_unrank are inverses of each other, built in one pass: walking
        // `l` upwards visits the values of each popcount in ascending order, so the
        // running counter per popcount *is* the rank, and the position it is pushed
        // to is that rank.
        let mut low_rank = vec![0u16; 1 << LOW_BITS];
        let mut low_unrank: Vec<Vec<u16>> = (0..=LOW_BITS as usize)
            .map(|j| Vec::with_capacity(c[LOW_BITS as usize][j] as usize))
            .collect();
        for l in 0..(1u32 << LOW_BITS) {
            let j = l.count_ones() as usize;
            low_rank[l as usize] = low_unrank[j].len() as u16;
            low_unrank[j].push(l as u16);
        }
        Self {
            words,
            summary: zeroed_atomic_vec(SUMMARY_WORDS),
            low_rank,
            low_unrank,
            high_cum: vec![0u32; HIGH_ENTRIES],
            // no layout yet: `begin_round` must run before any key is indexed, and
            // a peg count no board can have makes forgetting it fail the assertions
            // in `index` rather than silently mis-rank.
            pegs: usize::MAX,
        }
    }

    /// Empties the map and switches it to the layer of keys with `pegs` pegs, which
    /// is what makes [`Self::index`] a bijection for the round about to run.
    ///
    /// Clears rather than requiring the caller to: the stored bits are positions in
    /// the *old* layer's ranking, so carrying them across a change of `pegs` would
    /// silently reinterpret them as other boards. Making the clear part of the switch
    /// removes the chance of getting that order wrong. Clearing an already-empty map
    /// costs one scan of the (35 KiB) summary.
    pub(crate) fn begin_round(&mut self, pegs: usize) {
        self.clear();
        let c = binomials();
        assert!(pegs <= KEY_BITS, "a board cannot hold {pegs} pegs");
        assert!(
            c[KEY_BITS][pegs] as usize <= NUM_WORDS * 64,
            "layer of {pegs} pegs needs {} bits, map holds {}",
            c[KEY_BITS][pegs],
            NUM_WORDS * 64
        );
        // high_cum[h] = keys with this peg count below prefix h. A prefix that has
        // already used more than `pegs` bits, or too few to be completed by the low
        // half, contributes nothing.
        let mut acc = 0u64;
        for (h, slot) in self.high_cum.iter_mut().enumerate() {
            *slot = acc as u32;
            let used = (h as u64).count_ones() as usize;
            if used <= pegs && pegs - used <= LOW_BITS as usize {
                acc += c[LOW_BITS as usize][pegs - used];
            }
        }
        debug_assert_eq!(acc, c[KEY_BITS][pegs], "high_cum must total C(33, pegs)");
        self.pegs = pegs;
    }

    /// Position of `key` in this round's layer: its rank among the keys of the same
    /// popcount, which is dense in `0..C(33, pegs)` where the raw key is spread over
    /// `2^33`. See [`LOW_BITS`] for the two-table form.
    #[inline]
    fn index(&self, key: u64) -> u64 {
        debug_assert!(key < 1 << KEY_BITS, "key {key:#x} is wider than the board");
        debug_assert_eq!(
            key.count_ones() as usize,
            self.pegs,
            "key {key:#x} is not from this round's layer - its rank would collide \
             with another board's"
        );
        self.high_cum[(key >> LOW_BITS) as usize] as u64
            + self.low_rank[(key & LOW_MASK) as usize] as u64
    }

    /// Inverse of [`Self::index`], as a free function so that
    /// [`Self::extract_sorted_by_key`] - which has the fields borrowed apart - and
    /// the tests share one implementation. See [`unindex`].
    #[cfg(test)]
    fn unindex_key(&self, index: u64, hint: usize) -> (u64, usize) {
        unindex(&self.high_cum, &self.low_unrank, self.pegs, index, hint)
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
        let bit = self.index(key);
        let word_idx = (bit >> 6) as usize;
        let mask = 1u64 << (bit & 63);
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
        // The summary is the opposite case, and this is where the win is. Every
        // thread writes it, so an unconditional `fetch_or` here is a stream of
        // contended cross-core ownership transfers; every block that will ever be
        // non-empty gets marked within its first few keys, so this load almost
        // always finds the bit already set and skips the RMW entirely - keeping
        // the line Shared instead of bouncing it. Worth ~24-29ms per bitset round
        // here, measured back when the summary was 32 KiB and comfortably
        // L1-resident. It is 256 KiB since `BLOCK_WORDS` dropped to 64 (see the
        // trade documented there), so the load now generally comes from L2 rather
        // than L1 - which is exactly the cost that sweep was weighing, and still
        // nothing next to the DRAM round trip the `fetch_or` above pays.
        if sword.load(Ordering::Relaxed) & smask == 0 {
            sword.fetch_or(smask, Ordering::Relaxed);
        }
    }

    /// Starts fetching the cache line [`Self::set`] would touch for `key`, without
    /// waiting for it.
    ///
    /// This is the counterpart to the "don't guard the `fetch_or` with a load"
    /// comment above, and the reason that guard lost while this wins. `set` scatters
    /// randomly over a map far larger than cache, so most calls still miss - but the
    /// cost that dominates is not the miss, it is that a
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
        self.prefetch_word(key);
    }

    /// Starts fetching the cache line [`Self::test`] would read for `key`.
    ///
    /// Same single line as [`Self::prefetch_for_set`] - `set` and `test` index
    /// `words` identically - but it earns its keep for a different reason, so the
    /// two are named apart at the call sites. `test` is a plain `Relaxed` load
    /// with no `lock` prefix, so unlike `set` it is not itself a barrier and the
    /// core is free to overlap consecutive probes on its own. What limits that is
    /// how far ahead the out-of-order window can run, and the caller
    /// (`try_bitset_shrink_round`'s filter) puts a thoroughly unpredictable
    /// data-dependent branch on every probe - so the window keeps being spent on
    /// mispredicted work instead of on getting the next miss started.
    #[inline]
    pub(crate) fn prefetch_for_test(&self, key: u64) {
        self.prefetch_word(key);
    }

    #[inline]
    fn prefetch_word(&self, key: u64) {
        #[cfg(target_arch = "x86_64")]
        {
            // `index` maps into this round's layer, whose size `begin_round` has
            // checked against the map - the same bound `set`/`test` index under.
            let word_idx = (self.index(key) >> 6) as usize;
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
        let bit = self.index(key);
        let word_idx = (bit >> 6) as usize;
        (self.words[word_idx].load(Ordering::Relaxed) >> (bit & 63)) & 1 != 0
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

    /// clears every key that was set. Cheaper than clearing the whole map when
    /// occupancy is low (it always is here - even the biggest round sets ~5M of the
    /// 1.17G bits its layer spans):
    /// the summary tells us exactly which blocks need clearing, so untouched blocks
    /// (the vast majority) are skipped entirely.
    pub(crate) fn clear(&mut self) {
        let Self { words, summary, .. } = self;
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
    /// Rank order and key order agree - the rank counts the keys below its own -
    /// so this is still ascending by compressed key. Skips whole zero blocks via the
    /// summary rather than scanning the entire map.
    pub(crate) fn extract_sorted_by_key(&mut self) -> Vec<Board> {
        // `&mut` lets this read plain `u64`s rather than `Relaxed` atomics; see
        // `as_plain`. It matters most for the counting pass below, which is a
        // popcount over every word of every touched block - vectorizable as plain
        // loads, not as atomic ones.
        let Self {
            words,
            summary,
            high_cum,
            low_unrank,
            pegs,
            ..
        } = self;
        let pegs = *pegs;
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
                // Set bits are positions in this layer's ranking, so each one has to
                // be turned back into a key. Indices within a chunk only ever
                // increase and `high_cum` is monotone, so instead of searching it per
                // key a cursor walks forward - O(1) amortised, with one search to
                // find where this chunk starts. Total cursor travel across all chunks
                // is bounded by `high_cum`'s length, not by the number of keys.
                let first_index = (sword_idx * CHUNK_WORDS * 64) as u64;
                let mut h = high_cum
                    .partition_point(|&c| c as u64 <= first_index)
                    .saturating_sub(1);
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
                            let index = ((word_idx as u64) << 6) | bit as u64;
                            let key;
                            (key, h) = unindex(high_cum, low_unrank, pegs, index, h);
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

    /// Next value with the same popcount, ascending (Gosper's hack). Successive keys
    /// of one layer have successive *ranks*, which is what makes these useful for
    /// exercising same-word collisions.
    fn next_in_layer(v: u64) -> u64 {
        let c = v & v.wrapping_neg();
        let r = v + c;
        (((r ^ v) >> 2) / c) | r
    }

    /// every key of the `pegs` layer, ascending. Only tractable for small `pegs`.
    fn layer_keys(pegs: usize) -> Vec<u64> {
        assert!(pegs > 0);
        let mut out = Vec::new();
        let mut v = (1u64 << pegs) - 1;
        while v < 1 << KEY_BITS {
            out.push(v);
            v = next_in_layer(v);
        }
        out
    }

    /// `n` keys of the `pegs` layer, `stride` apart in rank, from the bottom.
    fn layer_sample(pegs: usize, n: usize, stride: usize) -> Vec<u64> {
        let mut out = Vec::with_capacity(n);
        let mut v = (1u64 << pegs) - 1;
        'outer: while out.len() < n {
            out.push(v);
            for _ in 0..stride {
                v = next_in_layer(v);
                if v >= 1 << KEY_BITS {
                    break 'outer;
                }
            }
        }
        out
    }

    /// The textbook rank: sum `C(p, i)` over the set bits, `i` counting from 1. An
    /// independent implementation of what the two-table form computes.
    fn colex_rank(mut key: u64, c: &[[u64; KEY_BITS + 1]; KEY_BITS + 1]) -> u64 {
        let mut rank = 0;
        let mut i = 1;
        while key != 0 {
            rank += c[key.trailing_zeros() as usize][i];
            key &= key - 1;
            i += 1;
        }
        rank
    }

    #[test]
    fn binomials_are_right() {
        let c = binomials();
        assert_eq!(c[KEY_BITS][0], 1);
        assert_eq!(c[KEY_BITS][1], 33);
        assert_eq!(c[KEY_BITS][2], 528);
        assert_eq!(c[16][8], 12_870);
        // the sizing constant, and the claim that 16 is the peak layer
        assert_eq!(c[KEY_BITS][16] as usize, MAX_LAYER_KEYS);
        assert_eq!(c[KEY_BITS][17] as usize, MAX_LAYER_KEYS);
        assert_eq!(
            (0..=KEY_BITS).map(|k| c[KEY_BITS][k]).max().unwrap() as usize,
            MAX_LAYER_KEYS,
            "MAX_LAYER_KEYS must bound every layer, or the map is too small"
        );
        // and the row must account for the whole key space
        assert_eq!(
            (0..=KEY_BITS).map(|k| c[KEY_BITS][k]).sum::<u64>(),
            1u64 << KEY_BITS
        );
    }

    #[test]
    fn map_holds_every_layer() {
        let c = binomials();
        for pegs in 0..=KEY_BITS {
            assert!(
                c[KEY_BITS][pegs] <= (NUM_WORDS * 64) as u64,
                "layer of {pegs} pegs does not fit"
            );
        }
    }

    #[test]
    fn index_is_a_bijection_onto_the_layer() {
        // exhaustive for the layers small enough to enumerate: every key must get a
        // distinct index, and together they must cover 0..C(33, pegs) exactly. A
        // collision here would silently merge two boards in the map.
        let c = binomials();
        for pegs in [1usize, 2, 3] {
            let mut set = DenseKeySet::new();
            set.begin_round(pegs);
            let keys = layer_keys(pegs);
            assert_eq!(keys.len() as u64, c[KEY_BITS][pegs]);
            let mut seen: Vec<u64> = keys.iter().map(|&k| set.index(k)).collect();
            assert!(
                seen.windows(2).all(|w| w[0] < w[1]),
                "index must be strictly increasing in the key, so rank order is key order"
            );
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), keys.len(), "indices collide for pegs={pegs}");
            assert_eq!(*seen.first().unwrap(), 0);
            assert_eq!(*seen.last().unwrap(), c[KEY_BITS][pegs] - 1);
        }
    }

    #[test]
    fn index_matches_the_textbook_rank_and_round_trips() {
        let c = binomials();
        for pegs in [1usize, 2, 5, 16, 17, 32, 33] {
            let mut set = DenseKeySet::new();
            set.begin_round(pegs);
            for key in layer_sample(pegs, 500, 977) {
                let index = set.index(key);
                assert_eq!(index, colex_rank(key, &c), "pegs={pegs} key={key:#x}");
                assert!(index < c[KEY_BITS][pegs]);
                // both from the bottom and with a hint, since extraction uses hints
                assert_eq!(set.unindex_key(index, 0).0, key, "round trip from 0");
                let hint = (key >> LOW_BITS) as usize;
                assert_eq!(set.unindex_key(index, hint).0, key, "round trip from hint");
            }
        }
    }

    #[test]
    fn set_test_roundtrip() {
        let pegs = 16;
        let mut set = DenseKeySet::new();
        set.begin_round(pegs);
        let keys = layer_sample(pegs, 64, 1);
        for &k in &keys {
            set.set(k);
        }
        for &k in &keys {
            assert!(set.test(k), "key {k:#x} should be set");
        }
        // absent keys of the same layer, past the ones written
        for k in layer_sample(pegs, 4, 1_000_000).into_iter().skip(1) {
            assert!(!set.test(k), "key {k:#x} was never set");
        }
    }

    #[test]
    fn concurrent_set_loses_no_bits() {
        let keys_per_thread = 5000;
        let nthreads = 8;
        let pegs = 16;
        let mut set = DenseKeySet::new();
        set.begin_round(pegs);
        // one dense run of the layer: consecutive keys have consecutive ranks, so
        // many land in the same 64-bit word and race on the same `fetch_or`
        let keys = layer_sample(pegs, keys_per_thread * nthreads, 1);
        std::thread::scope(|s| {
            for t in 0..nthreads {
                let set = &set;
                let keys = &keys;
                s.spawn(move || {
                    // interleaved so the threads' writes are spread over the run
                    for k in keys.iter().skip(t).step_by(nthreads) {
                        set.set(*k);
                    }
                });
            }
        });
        for &k in &keys {
            assert!(set.test(k), "key {k:#x} lost under concurrent set()");
        }
        assert_eq!(set.extract_sorted_by_key().len(), keys.len());
    }

    #[test]
    fn concurrent_set_spanning_many_blocks_loses_no_bits() {
        // The dense run above stays within a few blocks of one summary word. Spread
        // the keys over many blocks and many summary words instead, so a dropped
        // summary update makes whole runs vanish from extraction.
        let pegs = 16;
        let mut set = DenseKeySet::new();
        set.begin_round(pegs);
        // stride in rank so consecutive samples fall in different blocks
        let keys = layer_sample(pegs, 4000, BLOCK_WORDS * 64 + 1);
        let blocks: std::collections::HashSet<u64> = keys.iter().map(|&k| set.index(k) >> 12).collect();
        assert!(blocks.len() > 100, "want many distinct blocks, got {}", blocks.len());
        std::thread::scope(|s| {
            for t in 0..8 {
                let set = &set;
                let keys = &keys;
                s.spawn(move || {
                    for k in keys.iter().skip(t).step_by(8) {
                        set.set(*k);
                    }
                });
            }
        });
        let extracted: Vec<u64> = set
            .extract_sorted_by_key()
            .into_iter()
            .map(|b| b.to_compressed_repr())
            .collect();
        let mut expected = keys.clone();
        expected.sort_unstable();
        assert_eq!(extracted, expected);
    }

    #[test]
    fn word_bit_set_implies_summary_bit_set() {
        // Deterministic guard on the invariant `set()`'s conditional summary update
        // depends on, without relying on a race. Covers the first write to a block
        // and a repeat write (which skips the summary RMW).
        let pegs = 16;
        let mut set = DenseKeySet::new();
        set.begin_round(pegs);
        let summary_bit_set = |key: u64| {
            let block = (set.index(key) >> 6) as usize / BLOCK_WORDS;
            set.summary[block / 64].load(Ordering::Relaxed) & (1u64 << (block % 64)) != 0
        };
        let mut keys = layer_sample(pegs, 8, BLOCK_WORDS * 64 * 37 + 1);
        // include the extremes of the layer, whose indices are 0 and C(33,pegs)-1
        keys.push((1u64 << pegs) - 1);
        keys.push(((1u64 << pegs) - 1) << (KEY_BITS - pegs));
        for key in keys {
            set.set(key);
            assert!(set.test(key), "word bit missing for {key:#x}");
            assert!(summary_bit_set(key), "summary bit missing for {key:#x}");
            set.set(key);
            assert!(set.test(key), "word bit lost on repeat set of {key:#x}");
            assert!(summary_bit_set(key), "summary bit lost on repeat set of {key:#x}");
        }
    }

    #[test]
    fn extract_sorted_by_key_matches_and_is_ordered() {
        let pegs = 12;
        let mut set = DenseKeySet::new();
        set.begin_round(pegs);
        // spans many blocks and summary words, plus a pair straddling a block edge
        let mut keys = layer_sample(pegs, 300, CHUNK_WORDS * 64 / 7 + 1);
        let straddle = layer_sample(pegs, 2, 1);
        keys.extend(straddle);
        keys.push((1u64 << pegs) - 1);
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
        assert!(extracted.is_sorted(), "rank order must be key order");
    }

    #[test]
    fn begin_round_clears_the_previous_layer() {
        // Bits are positions in the layer that wrote them, so they must not survive
        // into a round that would read them as different boards.
        let mut set = DenseKeySet::new();
        set.begin_round(16);
        let old = layer_sample(16, 50, 3);
        for &k in &old {
            set.set(k);
        }
        assert_eq!(set.extract_sorted_by_key().len(), old.len());

        set.begin_round(15);
        assert!(
            set.extract_sorted_by_key().is_empty(),
            "the previous layer's bits survived a begin_round"
        );
        let new = layer_sample(15, 10, 5);
        for &k in &new {
            set.set(k);
        }
        let extracted: Vec<u64> = set
            .extract_sorted_by_key()
            .into_iter()
            .map(|b| b.to_compressed_repr())
            .collect();
        assert_eq!(extracted, new);
    }

    #[test]
    fn clear_removes_everything() {
        let pegs = 16;
        let mut set = DenseKeySet::new();
        set.begin_round(pegs);
        let keys = layer_sample(pegs, 16, 1_000);
        for &k in &keys {
            set.set(k);
        }
        assert!(!set.extract_sorted_by_key().is_empty());
        set.clear();
        assert!(set.extract_sorted_by_key().is_empty());
        for k in keys {
            assert!(!set.test(k));
        }
    }

    #[test]
    fn empty_set_extracts_nothing() {
        let mut set = DenseKeySet::new();
        set.begin_round(16);
        assert!(set.extract_sorted_by_key().is_empty());
    }
}
