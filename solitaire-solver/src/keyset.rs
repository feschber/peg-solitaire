//! Dense, fixed-size concurrent bitsets over `Board`'s compressed key space - one per
//! BFS layer - which are the whole of `calculate_feasible_set`'s working state.
//!
//! Every board in a BFS round has the same peg count `k`, so a layer's keys are the
//! `C(33, k)` patterns of that popcount - and, since every board the solver stores
//! carries [`Board::INVARIANT_TARGET`], only one in sixteen of those. A key is
//! indexed by its *rank* among exactly that set ([`LayerView::index`]), which is
//! dense in `0..layer_keys`, so a layer's bitmap is `C(33, k) / 16` bits: 8.7 MiB
//! for the peak layer and 32 MiB for all sixteen together. That is small enough to
//! simply keep every layer for the whole run, which is what [`KeySetPool`] does.
//!
//! Keeping them is what makes the solver's second phase cheap. The growth phase
//! writes each layer once and never extracts it: the next round *scans* it (see
//! [`LayerView::par_scan`]) and generates straight from the set bits. The shrink
//! phase then answers "is this candidate a board that can reach the solved
//! position?" with one probe into the candidate's layer, and takes the bit as it
//! goes ([`LayerView::take_at`]) so that every feasible board is emitted exactly
//! once without any sort or dedup.
//!
//! The price of ranking is that a key only has a meaningful position *within* one
//! layer: a view is bound to a peg count, and `index` asserts the popcount in debug
//! builds. Mixing layers would silently alias distinct boards onto one bit rather
//! than fail, which is why the views carry their layer and callers never see a raw
//! word offset.

use std::sync::{
    OnceLock,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

use rayon::prelude::*;

use crate::Board;

/// number of bits in `Board::to_compressed_repr`'s output.
const KEY_BITS: usize = Board::SLOTS;

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
///
/// Swept on a verified native build (13 reps, order rotated): 14 and 15 are
/// reliably worse (a 1-2 MiB `high_cum` stops fitting the cache the hot ranking
/// path needs it in), and 17/18 hint at a gain that does not survive a focused
/// paired test. Raising it is not a free constant change either: `low_unrank`
/// holds the low half's *values*, which stop fitting `u16` past 16 bits.
const LOW_BITS: u32 = 16;
const LOW_MASK: u64 = (1 << LOW_BITS) - 1;
const HIGH_ENTRIES: usize = 1 << (KEY_BITS as u32 - LOW_BITS);

/// words per summary bit: each summary bit says "is this whole 64-word (4 Kbit)
/// span of `words` entirely zero?", so a scan can skip it in O(1).
///
/// Swept when the summary was also what `clear` walked (it no longer clears
/// anything - a layer is written once and kept): coarser blocks shrink the summary
/// and keep `set_at`'s check of it in L1, finer ones make scans skip more. 64 was
/// the turning point then and there is no reason to revisit it now that the
/// summary only guides scans.
const BLOCK_WORDS: usize = 64;
/// bits per summary bit.
const BLOCK_BITS: usize = BLOCK_WORDS * 64;
/// words covered by one summary word. A layer's bitmap is padded up to a multiple of
/// this, so every summary word covers a full span of words.
const CHUNK_WORDS: usize = BLOCK_WORDS * 64;

/// Blocks handed to one parallel task by [`LayerView::par_scan`]: 4 blocks are
/// 16 Kbit of bitmap, a few hundred boards at the peak layer's average density.
///
/// The density is anything but uniform, which is what sizes this. Normalization
/// takes the minimum of each board's eight symmetries, and a minimum of eight
/// leaves the high bits of the key clear far more often than not, so the boards
/// of a layer crowd into the low end of its rank space. A task has to be small
/// enough that the densest ones - many times the average - are still a fraction of
/// a millisecond, since the last task to finish is what every round waits for.
/// Four blocks keeps that tail short while leaving the generator's prefetch ring
/// full across the task and the per-task overhead small next to its work.
const BLOCKS_PER_TASK: usize = 4;

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

fn zeroed_atomic_vec(len: usize) -> Vec<AtomicU64> {
    let zeros: Vec<u64> = vec![0u64; len];
    // SAFETY: `AtomicU64` is documented to have the same size, alignment, and bit
    // validity as `u64` (std::sync::atomic::AtomicU64 docs), so a `Vec<u64>` and a
    // `Vec<AtomicU64>` of the same length have identical layout; this only reinterprets
    // the backing allocation, it doesn't touch it. Using this instead of e.g.
    // `(0..len).map(|_| AtomicU64::new(0)).collect()` lets `vec![0u64; len]` satisfy
    // the allocation from the allocator's zeroed-memory path (typically fresh,
    // already-zero OS pages) instead of writing every word by hand.
    unsafe { std::mem::transmute::<Vec<u64>, Vec<AtomicU64>>(zeros) }
}

/// Reinterprets an exclusively-borrowed atomic slice as plain `u64`s.
///
/// Atomics are only needed while [`LayerView::set_at`] is running concurrently.
/// The bulk operations run between joined parallel regions, and `&mut` proves it -
/// so they can use plain loads and stores, which, unlike even `Relaxed` atomics,
/// LLVM can vectorize and fold.
fn as_plain(v: &mut [AtomicU64]) -> &mut [u64] {
    // SAFETY: `AtomicU64` is documented to have the same size, alignment and
    // bit validity as `u64`, so the reinterpretation is layout-valid; and the
    // `&mut` borrow proves no other thread can be accessing this memory, so
    // non-atomic access here cannot race. Same guarantee `zeroed_atomic_vec`
    // relies on, in the other direction.
    unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr().cast::<u64>(), v.len()) }
}

/// The layer-independent half of the ranking, built once and shared by every layer.
pub(crate) struct RankTables {
    /// `low_rank[l]` = how many 16-bit values below `l` share both its popcount *and* its
    /// invariant state.
    ///
    /// Narrowing the class from popcount alone to popcount-and-state is the whole of the
    /// invariant saving, and it costs [`LayerView::index`] nothing: the state is a
    /// property of `l`, so it is folded into this table at build time and the hot path is
    /// the same two lookups and an add it always was.
    ///
    /// A fixed-size array rather than a `Vec`, so that indexing it by the masked low
    /// half needs no bounds check - this is on the path of every key the solver ranks.
    low_rank: Box<[u16; 1 << LOW_BITS]>,
    /// The 16-bit values grouped by (popcount `j`, invariant state), each group
    /// ascending, laid end to end: the inverse of `low_rank`, needed only by the
    /// unranking direction. Group `(j, state)` starts at `low_unrank_off[j][state]`
    /// and has `low_count[j][state]` entries.
    ///
    /// One flat array rather than `Vec<Vec<Vec<u16>>>`, which is what it was: that
    /// put three dependent pointer chases on every board a scan decodes, and scans
    /// decode every board the solver ever generates from.
    low_unrank: Box<[u16; 1 << LOW_BITS]>,
    low_unrank_off: [[u32; 16]; LOW_BITS as usize + 1],
    /// `low_count[j][state]` = how many 16-bit values have popcount `j` and invariant
    /// state `state`.
    low_count: [[u32; 16]; LOW_BITS as usize + 1],
    /// `high_state[h]` = the invariant state of the prefix `h << LOW_BITS`.
    ///
    /// Precomputed because `Board::invariant_state` is a loop over the prefix's set
    /// bits, and [`Self::high_cum`] wants it for all `HIGH_ENTRIES` prefixes of every
    /// layer. One byte per prefix, 128 KiB.
    high_state: Vec<u8>,
}

impl RankTables {
    /// The tables, built on first use and kept for the life of the process: they
    /// depend on nothing but the board, and building them is a pass over all `2^16`
    /// low halves - ~0.4 ms that would otherwise be paid by every solve. ~400 KiB.
    fn get() -> &'static Self {
        static TABLES: OnceLock<RankTables> = OnceLock::new();
        TABLES.get_or_init(Self::new)
    }

    fn new() -> Self {
        // low_rank / low_unrank are inverses of each other: walking `l` upwards visits
        // the values of each (popcount, state) class in ascending order, so the running
        // count of the class *is* the rank, and the rank is the position within the
        // class's group of `low_unrank`.
        let mut low_rank = vec![0u16; 1 << LOW_BITS];
        let mut low_count = [[0u32; 16]; LOW_BITS as usize + 1];
        let class = |l: u32| (l.count_ones() as usize, Board::invariant_state(l as u64) as usize);
        for l in 0..(1u32 << LOW_BITS) {
            let (j, state) = class(l);
            low_rank[l as usize] = low_count[j][state] as u16;
            low_count[j][state] += 1;
        }
        // groups laid end to end in (popcount, state) order; the counts are exact, so
        // the groups partition the array and the last one ends at its top
        let mut low_unrank_off = [[0u32; 16]; LOW_BITS as usize + 1];
        let mut next = 0u32;
        for j in 0..=LOW_BITS as usize {
            for state in 0..16 {
                low_unrank_off[j][state] = next;
                next += low_count[j][state];
            }
        }
        debug_assert_eq!(next, 1 << LOW_BITS);
        let mut low_unrank = vec![0u16; 1 << LOW_BITS];
        for l in 0..(1u32 << LOW_BITS) {
            let (j, state) = class(l);
            low_unrank[(low_unrank_off[j][state] + u32::from(low_rank[l as usize])) as usize] =
                l as u16;
        }
        let c = binomials();
        debug_assert!(
            (0..=LOW_BITS as usize).all(|j| low_count[j].iter().map(|&n| u64::from(n)).sum::<u64>()
                == c[LOW_BITS as usize][j]),
            "the 16 states of each popcount must partition its C(16, j) values"
        );
        Self {
            low_rank: low_rank
                .into_boxed_slice()
                .try_into()
                .expect("built with exactly 1 << LOW_BITS entries"),
            low_unrank: low_unrank
                .into_boxed_slice()
                .try_into()
                .expect("built with exactly 1 << LOW_BITS entries"),
            low_unrank_off,
            low_count,
            high_state: (0..HIGH_ENTRIES)
                .map(|h| Board::invariant_state((h as u64) << LOW_BITS))
                .collect(),
        }
    }

    /// The layer-dependent half of the ranking for the layer of `pegs` pegs:
    /// `high_cum[h]` = keys of that layer below the prefix `h << LOW_BITS`, plus the
    /// layer's total.
    fn high_cum(&self, pegs: usize) -> (Box<[u32; HIGH_ENTRIES]>, u64) {
        assert!(pegs <= KEY_BITS, "a board cannot hold {pegs} pegs");
        let mut high_cum: Box<[u32; HIGH_ENTRIES]> = vec![0u32; HIGH_ENTRIES]
            .into_boxed_slice()
            .try_into()
            .expect("built with exactly HIGH_ENTRIES entries");
        let mut acc = 0u64;
        for (h, slot) in high_cum.iter_mut().enumerate() {
            *slot = acc as u32;
            let used = (h as u64).count_ones() as usize;
            // A prefix that has already used more than `pegs` bits, or too few to be
            // completed by the low half, contributes nothing.
            if used <= pegs && pegs - used <= LOW_BITS as usize {
                // Only the low halves that complete this prefix *to the target* count: the
                // state is a XOR, so the low half must carry whatever the prefix is missing.
                let wanted = self.high_state[h] ^ Board::INVARIANT_TARGET;
                acc += u64::from(self.low_count[pegs - used][wanted as usize]);
            }
        }
        debug_assert!(
            acc <= binomials()[KEY_BITS][pegs],
            "layer {pegs} totals {acc}, more than the C(33, {pegs}) keys of that popcount"
        );
        (high_cum, acc)
    }
}

/// The key whose rank within the `pegs` layer is `index` - the inverse of
/// [`LayerView::index`].
///
/// `hint` is a prefix index known not to exceed the answer's, so a caller decoding
/// ascending indices can walk `high_cum` forward instead of searching it per key;
/// pass 0 to start from the bottom. Returns the prefix it settled on, to be fed back
/// as the next `hint`.
fn unindex(high_cum: &[u32; HIGH_ENTRIES], tables: &RankTables, pegs: usize, index: u64, hint: usize) -> (u64, usize) {
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
    // same reasoning as `RankTables::high_cum`: this prefix's keys are exactly those
    // whose low half carries the state the prefix is missing
    let wanted = (tables.high_state[h] ^ Board::INVARIANT_TARGET) as usize;
    let within = (index - high_cum[h] as u64) as usize;
    debug_assert!(within < tables.low_count[pegs - used][wanted] as usize);
    let low = tables.low_unrank[tables.low_unrank_off[pegs - used][wanted] as usize + within];
    let key = ((h as u64) << LOW_BITS) | low as u64;
    debug_assert_eq!(key.count_ones() as usize, pegs);
    debug_assert_eq!(Board::invariant_state(key), Board::INVARIANT_TARGET);
    (key, h)
}

/// Where one layer lives inside the pool, and the ranking that maps its keys there.
struct LayerMeta {
    pegs: usize,
    /// how many keys the layer ranks onto, i.e. one past the largest index
    /// [`LayerView::index`] can return for it.
    #[cfg(test)]
    keys: u64,
    /// offset and length of the layer's span of `KeySetPool::words`; `num_words` is a
    /// multiple of `CHUNK_WORDS`, so the span covers whole summary words.
    word_off: usize,
    num_words: usize,
    /// offset of the layer's span of `KeySetPool::summary`.
    sum_off: usize,
    /// `high_cum[h]` = keys of this layer below the prefix `h << LOW_BITS`; the
    /// layer-dependent half of the ranking. Fixed-size for the same reason
    /// `RankTables::low_rank` is.
    high_cum: Box<[u32; HIGH_ENTRIES]>,
}

/// One bitset per BFS layer, plus one scratch bitset ranked like the largest layer,
/// all in a single allocation.
///
/// A layer is keyed by its peg count, `1..=max_pegs`. The scratch layer exists for
/// the one round that has to *mark* boards of the largest layer while still probing
/// that layer, so it cannot take bits from it as the other rounds do.
pub(crate) struct KeySetPool {
    words: Vec<AtomicU64>,
    summary: Vec<AtomicU64>,
    tables: &'static RankTables,
    /// indexed by peg count; `layers[0]` is an empty placeholder (no board of the
    /// invariant subspace has zero pegs), `layers[max_pegs + 1]` is the scratch layer.
    layers: Vec<LayerMeta>,
    max_pegs: usize,
}

impl KeySetPool {
    /// Allocates layers `1..=max_pegs` and the scratch layer.
    ///
    /// Deliberately not prefaulted: the zeroed allocation comes straight from the
    /// OS, and the growth phase touches each layer essentially everywhere anyway, so
    /// first touch is spread over the parallel writes that need it rather than paid
    /// up front on one thread.
    ///
    /// Nor backed by huge pages, though the pool looks like the textbook case for
    /// them - ~10K page faults per solve and a random scatter over 32 MiB of 4 KiB
    /// pages. Tried: the allocation over-sized by one huge page so the layers start
    /// on a 2 MiB boundary, `madvise(MADV_HUGEPAGE)` on the rest. The kernel backed
    /// only part of it (faults fell by a third, not to ~20), and the time did not
    /// move: -0.9 ms warm in 4 of 8 interleaved pinned runs, cold runs split both
    /// ways. Not worth a `libc` dependency for.
    pub(crate) fn new(max_pegs: usize) -> Self {
        let tables = RankTables::get();
        // one `high_cum` per layer, ~0.1 ms each: built in parallel rather than in
        // series, since the run this sets up for is only ~50 ms long
        let cums: Vec<(Box<[u32; HIGH_ENTRIES]>, u64)> = (0..=max_pegs + 1)
            .into_par_iter()
            .map(|slot| tables.high_cum(slot.min(max_pegs)))
            .collect();
        let mut layers = Vec::with_capacity(max_pegs + 2);
        let mut word_off = 0;
        for (slot, (high_cum, keys)) in cums.into_iter().enumerate() {
            let num_words = (keys as usize).div_ceil(64).next_multiple_of(CHUNK_WORDS);
            debug_assert!(keys as usize <= num_words * 64);
            layers.push(LayerMeta {
                pegs: slot.min(max_pegs),
                #[cfg(test)]
                keys,
                word_off,
                num_words,
                sum_off: word_off / CHUNK_WORDS,
                high_cum,
            });
            word_off += num_words;
        }
        Self {
            words: zeroed_atomic_vec(word_off),
            summary: zeroed_atomic_vec(word_off / CHUNK_WORDS),
            tables,
            layers,
            max_pegs,
        }
    }

    fn view(&self, slot: usize) -> LayerView<'_> {
        let meta = &self.layers[slot];
        LayerView {
            words: &self.words[meta.word_off..meta.word_off + meta.num_words],
            summary: &self.summary[meta.sum_off..meta.sum_off + meta.num_words / CHUNK_WORDS],
            high_cum: &meta.high_cum,
            tables: self.tables,
            pegs: meta.pegs,
            #[cfg(test)]
            keys: meta.keys,
        }
    }

    /// The layer of boards with `pegs` pegs.
    pub(crate) fn layer(&self, pegs: usize) -> LayerView<'_> {
        assert!(
            (1..=self.max_pegs).contains(&pegs),
            "no layer for {pegs} pegs in a pool of 1..={}",
            self.max_pegs
        );
        self.view(pegs)
    }

    /// The scratch layer, ranked like the layer of `max_pegs` pegs.
    pub(crate) fn scratch(&self) -> LayerView<'_> {
        self.view(self.max_pegs + 1)
    }

    /// Extracts every set key of the scratch layer as a `Board`, in ascending
    /// compressed-key order, emptying it.
    ///
    /// Rank order and key order agree - the rank counts the keys below its own - so
    /// this is ascending by compressed key. Skips whole zero blocks via the summary.
    pub(crate) fn drain_scratch(&mut self) -> Vec<Board> {
        let slot = self.max_pegs + 1;
        let meta = &self.layers[slot];
        let (word_off, num_words, sum_off) = (meta.word_off, meta.num_words, meta.sum_off);
        let words = as_plain(&mut self.words[word_off..word_off + num_words]);
        let summary = as_plain(&mut self.summary[sum_off..sum_off + num_words / CHUNK_WORDS]);
        let high_cum = &self.layers[slot].high_cum;
        let tables = self.tables;
        let pegs = self.layers[slot].pegs;
        // One chunk per summary word, so each worker owns the words its summary word
        // covers and can zero them as it reads them.
        let chunks: Vec<Vec<Board>> = words
            .par_chunks_mut(CHUNK_WORDS)
            .zip(summary.par_iter_mut())
            .enumerate()
            .map(|(sword_idx, (chunk, sword))| {
                let bits = std::mem::replace(sword, 0);
                if bits == 0 {
                    return Vec::new();
                }
                // count first so this chunk's `Vec` is allocated exactly once
                let mut count = 0usize;
                let mut b = bits;
                while b != 0 {
                    let block = b.trailing_zeros() as usize * BLOCK_WORDS;
                    for w in &chunk[block..block + BLOCK_WORDS] {
                        count += w.count_ones() as usize;
                    }
                    b &= b - 1;
                }
                let mut out = Vec::with_capacity(count);
                // Indices within a chunk only ever increase and `high_cum` is monotone,
                // so a cursor walks it forward instead of searching per key.
                let first_index = (sword_idx * CHUNK_WORDS * 64) as u64;
                let mut h = high_cum
                    .partition_point(|&c| c as u64 <= first_index)
                    .saturating_sub(1);
                let mut b = bits;
                while b != 0 {
                    let block_in_chunk = b.trailing_zeros() as usize * BLOCK_WORDS;
                    for (wi, w) in chunk[block_in_chunk..block_in_chunk + BLOCK_WORDS]
                        .iter_mut()
                        .enumerate()
                    {
                        let mut wbits = std::mem::replace(w, 0);
                        let word_idx = sword_idx * CHUNK_WORDS + block_in_chunk + wi;
                        while wbits != 0 {
                            let index = ((word_idx as u64) << 6) | wbits.trailing_zeros() as u64;
                            let key;
                            (key, h) = unindex(high_cum, tables, pegs, index, h);
                            out.push(Board::from_compressed_repr(key));
                            wbits &= wbits - 1;
                        }
                    }
                    b &= b - 1;
                }
                out
            })
            .collect();
        crate::par::par_join_owned(chunks)
    }
}

/// A shared view of one layer's bitset and ranking: what the hot loops hold.
///
/// `Copy`, so a parallel closure can capture it by value; it is a handful of slices.
#[derive(Clone, Copy)]
pub(crate) struct LayerView<'a> {
    words: &'a [AtomicU64],
    summary: &'a [AtomicU64],
    high_cum: &'a [u32; HIGH_ENTRIES],
    tables: &'a RankTables,
    pegs: usize,
    #[cfg(test)]
    keys: u64,
}

impl<'a> LayerView<'a> {
    /// Position of `key` in this layer: its rank among the keys of the same popcount
    /// that carry the invariant, which is dense in `0..keys`. See [`LOW_BITS`] for the
    /// two-table form.
    ///
    /// Callers that both prefetch and then act on the same key compute this once and
    /// pass the result to the `*_at` methods, rather than having each of them
    /// re-derive it: the two table reads and their bounds checks profiled at ~5.9% of
    /// a run when done twice.
    #[inline]
    pub(crate) fn index(&self, key: u64) -> u64 {
        debug_assert!(key < 1 << KEY_BITS, "key {key:#x} is wider than the board");
        debug_assert_eq!(
            key.count_ones() as usize,
            self.pegs,
            "key {key:#x} is not from the {} layer - its rank would collide with \
             another board's",
            self.pegs
        );
        debug_assert_eq!(
            Board::invariant_state(key),
            Board::INVARIANT_TARGET,
            "key {key:#x} is outside the invariant subspace the ranking is a bijection on"
        );
        // both indices are masked to the tables' fixed sizes, so neither read needs
        // a bounds check; the mask on the high half is a no-op for any real key
        self.high_cum[(key >> LOW_BITS) as usize & (HIGH_ENTRIES - 1)] as u64
            + self.tables.low_rank[(key & LOW_MASK) as usize] as u64
    }

    /// Inverse of [`Self::index`]; see [`unindex`] for what `hint` is for.
    #[inline]
    pub(crate) fn unrank(&self, index: u64, hint: usize) -> (u64, usize) {
        unindex(self.high_cum, self.tables, self.pegs, index, hint)
    }

    /// Marks the bit at a position obtained from [`Self::index`]. Safe to call from
    /// many threads at once.
    ///
    /// INVARIANT, relied on by every scan: if a bit in `words` is set then that bit's
    /// block is marked in `summary`. Every call reaches the summary code below, so
    /// each setter either marks the block itself or observes it already marked -
    /// which, by induction, means some call did mark it. Readers all run after the
    /// generating rayon region has joined, and that join supplies the happens-before
    /// edge making these `Relaxed` writes visible.
    #[inline]
    pub(crate) fn set_at(&self, bit: u64) {
        let word_idx = (bit >> 6) as usize;
        let mask = 1u64 << (bit & 63);
        // Written unconditionally, on purpose. Guarding this with a `load` first
        // looks attractive (~85% of the raw moves per round are duplicates of a key
        // already set) but measured worse than just writing: the line has to be
        // pulled in either way, so the load doesn't avoid the miss, it just adds a
        // second dependent access in front of it. And do not consume the return
        // value to count new keys: that makes LLVM emit a `lock cmpxchg` retry loop
        // instead of a single `lock or`, measured ~36 ms worse over ~34M calls.
        self.words[word_idx].fetch_or(mask, Ordering::Relaxed);

        let block = word_idx / BLOCK_WORDS;
        let sword = &self.summary[block / 64];
        let smask = 1u64 << (block % 64);
        // The summary is the opposite case. Every thread writes it, so an
        // unconditional `fetch_or` here is a stream of contended cross-core
        // ownership transfers; every block that will ever be non-empty gets marked
        // within its first few keys, so this load almost always finds the bit
        // already set and skips the RMW entirely - keeping the line Shared instead
        // of bouncing it. Worth ~24-29 ms per bitset round when measured.
        if sword.load(Ordering::Relaxed) & smask == 0 {
            sword.fetch_or(smask, Ordering::Relaxed);
        }
    }

    /// Starts fetching the cache line [`Self::set_at`]/[`Self::test_at`] would
    /// touch for `bit`, without waiting for it.
    ///
    /// It earns its keep on the *write* side because a `lock`ed RMW is a full
    /// barrier: it drains the store buffer, so consecutive independent misses
    /// cannot overlap in the core's line-fill buffers and each pays the full
    /// latency in series. Issuing the fetch some distance ahead of the RMW restores
    /// the memory-level parallelism the `lock` prefix destroys.
    ///
    /// On the *read* side the argument is different, since `test_at` is a plain
    /// `Relaxed` load and no barrier at all - the core will overlap probes on its
    /// own. What limits it there is how far ahead the out-of-order window can run,
    /// and the probe loops put a thoroughly unpredictable data-dependent branch on
    /// every probe, so the window keeps being spent on mispredicted work instead of
    /// starting the next miss. Every caller pipelines this against its own work via
    /// a ring buffer.
    #[inline]
    pub(crate) fn prefetch_at(&self, bit: u64) {
        #[cfg(target_arch = "x86_64")]
        {
            let word_idx = (bit >> 6) as usize;
            debug_assert!(word_idx < self.words.len());
            // SAFETY: `word_idx` is in bounds per the above, so the pointer is within
            // the `words` allocation. A prefetch is a hint with no architectural
            // effect - it neither reads nor writes - so it cannot race with the
            // concurrent `set_at` calls happening around it.
            unsafe {
                core::arch::x86_64::_mm_prefetch::<{ core::arch::x86_64::_MM_HINT_T0 }>(
                    self.words.as_ptr().add(word_idx).cast::<i8>(),
                );
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        let _ = bit;
    }

    /// [`Self::prefetch_at`] for a line about to be written by [`Self::set_at`]:
    /// asks for the line with write intent, so it arrives owned rather than
    /// shared and the `lock or` needs no second round trip to take ownership.
    ///
    /// `prefetchw` by inline asm rather than `_mm_prefetch` with `_MM_HINT_ET0`:
    /// LLVM only lowers that hint to `prefetchw` when the `prfchw` target feature
    /// is on, which `x86-64-v3` does not include, and silently emits `prefetcht0`
    /// otherwise. The instruction itself predates the feature bit - every x86-64
    /// CPU either executes it or (Intel before Broadwell) treats it as a NOP - so
    /// emitting it unconditionally cannot fault anywhere this binary runs.
    #[inline]
    pub(crate) fn prefetch_for_write_at(&self, bit: u64) {
        #[cfg(target_arch = "x86_64")]
        {
            let word_idx = (bit >> 6) as usize;
            debug_assert!(word_idx < self.words.len());
            // SAFETY: in bounds per the above; a prefetch has no architectural
            // effect - it writes nothing, so `readonly` is accurate - and it
            // cannot race with anything.
            unsafe {
                core::arch::asm!(
                    "prefetchw [{p}]",
                    p = in(reg) self.words.as_ptr().add(word_idx),
                    options(readonly, nostack, preserves_flags)
                );
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        self.prefetch_at(bit);
    }

    /// Tests the bit at a position obtained from [`Self::index`].
    #[inline]
    pub(crate) fn test_at(&self, bit: u64) -> bool {
        let word_idx = (bit >> 6) as usize;
        (self.words[word_idx].load(Ordering::Relaxed) >> (bit & 63)) & 1 != 0
    }

    /// Clears the bit at a position obtained from [`Self::index`], returning whether
    /// it was set - so that of any number of concurrent callers, exactly one gets
    /// `true`. That is what lets the shrink phase emit each board it finds exactly
    /// once without ever deduplicating.
    ///
    /// Callers test first (`test_at`) and only come here on a hit, so the RMW - and
    /// the ownership transfer it costs - is paid once per *distinct* board found,
    /// not once per probe.
    ///
    /// The summary is left alone: it may over-approximate (a marked block with no
    /// bits left), which every scan tolerates, and a layer that has been taken from
    /// is never scanned again anyway.
    #[inline]
    pub(crate) fn take_at(&self, bit: u64) -> bool {
        let word_idx = (bit >> 6) as usize;
        let mask = 1u64 << (bit & 63);
        self.words[word_idx].fetch_and(!mask, Ordering::Relaxed) & mask != 0
    }

    /// Convenience for tests, which think in keys; every hot path ranks the key
    /// itself and calls [`Self::set_at`] so the rank is computed once.
    #[cfg(test)]
    pub(crate) fn set(&self, key: u64) {
        self.set_at(self.index(key));
    }

    /// Convenience for tests; see [`Self::set`].
    #[cfg(test)]
    pub(crate) fn test(&self, key: u64) -> bool {
        self.test_at(self.index(key))
    }

    fn num_blocks(&self) -> usize {
        self.words.len() / BLOCK_WORDS
    }

    fn block_is_marked(&self, block: usize) -> bool {
        self.summary[block / 64].load(Ordering::Relaxed) & (1u64 << (block % 64)) != 0
    }

    /// Runs `f` over the layer's set keys in parallel, each call handed one
    /// [`Keys`] iterator over a contiguous run of [`BLOCKS_PER_TASK`] blocks.
    ///
    /// Boards come out in ascending key order within a task, and tasks cover
    /// disjoint, ascending ranges - which is what lets each task walk `high_cum`
    /// with a cursor rather than searching it per key.
    ///
    /// Callers pull the iterator one board *ahead* of the one they are working on.
    /// Decoding a bit back to a key is a short chain of dependent table loads behind
    /// an unpredictable cursor walk, and a board's ten-odd moves cannot start until
    /// it is done; the out-of-order window does not reach across a board's moves to
    /// overlap it with the next decode. Pulled one ahead, the decode runs under the
    /// previous board's moves instead. Without that, scanning cost ~3 cycles per
    /// generated move more than the extract-to-a-list pipeline it replaced, which
    /// did its decoding in a separate tight loop where consecutive decodes overlap.
    ///
    /// Split all the way down to single tasks (`with_max_len(1)`), not left to
    /// rayon's adaptive splitting. That splitting assumes the work is spread evenly
    /// over the range: it cuts the range into a few pieces per thread and only
    /// subdivides a piece further when it is stolen - a piece a thread is already
    /// working through is never split again. With the boards crowded into the low
    /// end of the range (see [`BLOCKS_PER_TASK`]), the thread that drew the dense
    /// piece then runs it alone while the others sit on empty ranges. Measured on
    /// the peak layer: the round used *less* CPU time than the list-based version
    /// it replaced and still took longer in wall time, until this.
    pub(crate) fn par_scan<F>(&self, f: F)
    where
        F: Fn(Keys<'a>) + Sync,
    {
        let view = *self;
        (0..self.num_blocks().div_ceil(BLOCKS_PER_TASK))
            .into_par_iter()
            .with_max_len(1)
            .for_each(|task| {
                let first = task * BLOCKS_PER_TASK;
                let end = ((task + 1) * BLOCKS_PER_TASK).min(view.num_blocks());
                // skip tasks whose blocks are all empty without building a cursor
                if (first..end).any(|b| view.block_is_marked(b)) {
                    f(Keys::new(view, first, end));
                }
            });
    }

    /// Every set key of the layer, ascending - the serial form of [`Self::par_scan`].
    #[cfg(test)]
    pub(crate) fn keys(&self) -> Keys<'a> {
        Keys::new(*self, 0, self.num_blocks())
    }

    /// Counts the layer's set bits. A full pass over the touched blocks.
    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        let view = *self;
        (0..self.num_blocks())
            .into_par_iter()
            .filter(|&b| view.block_is_marked(b))
            .map(|b| {
                view.words[b * BLOCK_WORDS..(b + 1) * BLOCK_WORDS]
                    .iter()
                    .map(|w| w.load(Ordering::Relaxed).count_ones() as usize)
                    .sum::<usize>()
            })
            .sum()
    }

    /// How many keys the layer can hold - its size.
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> u64 {
        self.keys
    }
}

/// Iterator over the set keys of a block range of one layer, ascending, as
/// `(index, key)` pairs - the index being the bit position [`LayerView::index`]
/// gives the key, so a caller can refer back to the bit without ranking again.
pub(crate) struct Keys<'a> {
    view: LayerView<'a>,
    /// next word to load
    word: usize,
    end: usize,
    /// the word the bits in `bits` came from
    cur: usize,
    bits: u64,
    cursor: usize,
}

impl<'a> Keys<'a> {
    fn new(view: LayerView<'a>, first_block: usize, end_block: usize) -> Self {
        let first_index = (first_block * BLOCK_BITS) as u64;
        let cursor = view
            .high_cum
            .partition_point(|&c| c as u64 <= first_index)
            .saturating_sub(1);
        Self {
            view,
            word: first_block * BLOCK_WORDS,
            end: end_block * BLOCK_WORDS,
            cur: 0,
            bits: 0,
            cursor,
        }
    }
}

impl Iterator for Keys<'_> {
    type Item = (u64, u64);

    #[inline]
    fn next(&mut self) -> Option<(u64, u64)> {
        while self.bits == 0 {
            if self.word >= self.end {
                return None;
            }
            // at a block boundary, the summary says whether the next 64 words hold
            // anything at all
            if self.word.is_multiple_of(BLOCK_WORDS)
                && !self.view.block_is_marked(self.word / BLOCK_WORDS)
            {
                self.word += BLOCK_WORDS;
                continue;
            }
            self.cur = self.word;
            self.bits = self.view.words[self.word].load(Ordering::Relaxed);
            self.word += 1;
        }
        let index = ((self.cur as u64) << 6) | self.bits.trailing_zeros() as u64;
        self.bits &= self.bits - 1;
        let (key, cursor) = self.view.unrank(index, self.cursor);
        self.cursor = cursor;
        Some((index, key))
    }
}

/// Counter a parallel scan can add to once per task rather than once per item.
pub(crate) struct Tally(AtomicUsize);

impl Tally {
    pub(crate) fn new() -> Self {
        Self(AtomicUsize::new(0))
    }
    pub(crate) fn add(&self, n: usize) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }
    pub(crate) fn get(&self) -> usize {
        self.0.load(Ordering::Relaxed)
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

    /// Keys of the `pegs` layer that the ranking actually covers, ascending.
    ///
    /// Filtered to [`Board::INVARIANT_TARGET`]: the ranking is a bijection onto those keys
    /// alone, and `index` rejects the rest.
    fn layer_iter(pegs: usize) -> impl Iterator<Item = u64> {
        assert!(pegs > 0);
        let mut v = (1u64 << pegs) - 1;
        core::iter::from_fn(move || {
            while v < 1 << KEY_BITS {
                let key = v;
                v = next_in_layer(v);
                if Board::invariant_state(key) == Board::INVARIANT_TARGET {
                    return Some(key);
                }
            }
            None
        })
    }

    /// every key of the `pegs` layer, ascending. Only tractable for small `pegs`.
    fn layer_keys(pegs: usize) -> Vec<u64> {
        layer_iter(pegs).collect()
    }

    /// `n` keys of the `pegs` layer, `stride` apart in rank, from the bottom.
    fn layer_sample(pegs: usize, n: usize, stride: usize) -> Vec<u64> {
        layer_iter(pegs).step_by(stride).take(n).collect()
    }

    /// `ways[i][j][s]` = ways to pick `j` of the positions below `i` whose weights XOR to `s`.
    fn ways_below() -> Vec<[[u64; 16]; KEY_BITS + 1]> {
        let mut ways = vec![[[0u64; 16]; KEY_BITS + 1]; KEY_BITS + 1];
        ways[0][0][0] = 1;
        for i in 0..KEY_BITS {
            let w = Board::INVARIANT_WEIGHTS[i] as usize;
            for j in 0..=KEY_BITS {
                for state in 0..16 {
                    ways[i + 1][j][state] =
                        ways[i][j][state] + if j == 0 { 0 } else { ways[i][j - 1][state ^ w] };
                }
            }
        }
        ways
    }

    /// Rank of `key` among its layer's *in-subspace* keys, the straightforward way.
    ///
    /// The independent reference for [`LayerView::index`]'s two-table form. Walks the
    /// bits from the top; at each set bit, every smaller key agreeing above it has a zero
    /// there, so it counts the completions below that carry whatever state the prefix
    /// still owes the target.
    fn subspace_rank(key: u64, pegs: usize, ways: &[[[u64; 16]; KEY_BITS + 1]]) -> u64 {
        let mut rank = 0u64;
        let mut remaining = pegs;
        let mut state = 0u8;
        for i in (0..KEY_BITS).rev() {
            if key >> i & 1 == 1 {
                let owed = (Board::INVARIANT_TARGET ^ state) as usize;
                rank += ways[i][remaining][owed];
                state ^= Board::INVARIANT_WEIGHTS[i];
                remaining -= 1;
            }
        }
        rank
    }

    /// The textbook popcount-only rank: sum `C(p, i)` over the set bits, `i` counting from 1.
    ///
    /// Restricting to a subset of the keys can only pull ranks down, so the subspace
    /// rank must never exceed this.
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

    fn extracted(view: LayerView<'_>) -> Vec<u64> {
        view.keys().map(|(_, key)| key).collect()
    }

    #[test]
    fn binomials_are_right() {
        let c = binomials();
        assert_eq!(c[KEY_BITS][0], 1);
        assert_eq!(c[KEY_BITS][1], 33);
        assert_eq!(c[KEY_BITS][2], 528);
        assert_eq!(c[16][8], 12_870);
        assert_eq!(c[KEY_BITS][16], 1_166_803_110);
        assert_eq!(c[KEY_BITS][17], 1_166_803_110);
        assert_eq!(
            (0..=KEY_BITS).map(|k| c[KEY_BITS][k]).sum::<u64>(),
            1u64 << KEY_BITS
        );
    }

    /// Every layer's size has to match the independent DP, the ranking's total has to
    /// be what the bitmap was sized for, and the layers must partition the invariant
    /// subspace.
    #[test]
    fn layer_sizes_match_the_dp() {
        let ways = ways_below();
        let target = Board::INVARIANT_TARGET as usize;
        let pool = KeySetPool::new(16);
        for pegs in 1..=16 {
            let view = pool.layer(pegs);
            assert_eq!(view.capacity(), ways[KEY_BITS][pegs][target], "layer {pegs}");
            assert!(view.capacity() as usize <= view.words.len() * 64);
        }
        assert_eq!(pool.scratch().capacity(), pool.layer(16).capacity());
        assert_eq!(pool.layer(16).capacity(), 72_922_839);
        assert_eq!(
            (0..=KEY_BITS)
                .map(|k| ways[KEY_BITS][k][target])
                .sum::<u64>(),
            1u64 << 29,
        );
    }

    #[test]
    fn index_is_a_bijection_onto_the_layer() {
        // exhaustive for the layers small enough to enumerate: every key must get a
        // distinct index, and together they must cover 0..keys exactly. A collision
        // here would silently merge two boards in the map.
        let pool = KeySetPool::new(3);
        for pegs in [1usize, 2, 3] {
            let view = pool.layer(pegs);
            let keys = layer_keys(pegs);
            assert_eq!(keys.len() as u64, view.capacity());
            assert!(!keys.is_empty(), "layer {pegs} came out empty");
            let mut seen: Vec<u64> = keys.iter().map(|&k| view.index(k)).collect();
            assert!(
                seen.windows(2).all(|w| w[0] < w[1]),
                "index must be strictly increasing in the key, so rank order is key order"
            );
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), keys.len(), "indices collide for pegs={pegs}");
            assert_eq!(*seen.first().unwrap(), 0);
            assert_eq!(*seen.last().unwrap(), keys.len() as u64 - 1);
        }
    }

    #[test]
    fn index_matches_the_textbook_rank_and_round_trips() {
        let ways = ways_below();
        let pool = KeySetPool::new(16);
        for pegs in [1usize, 2, 5, 16] {
            let view = pool.layer(pegs);
            for key in layer_sample(pegs, 500, 977) {
                let index = view.index(key);
                assert_eq!(
                    index,
                    subspace_rank(key, pegs, &ways),
                    "pegs={pegs} key={key:#x}"
                );
                assert!(index < view.capacity());
                assert!(
                    index <= colex_rank(key, &binomials()),
                    "subspace rank exceeds the popcount rank for {key:#x}"
                );
                // both from the bottom and with a hint, since scans use hints
                assert_eq!(view.unrank(index, 0).0, key, "round trip from 0");
                let hint = (key >> LOW_BITS) as usize;
                assert_eq!(view.unrank(index, hint).0, key, "round trip from hint");
            }
        }
    }

    #[test]
    fn set_test_take_roundtrip() {
        let pool = KeySetPool::new(16);
        let view = pool.layer(16);
        let keys = layer_sample(16, 64, 1);
        for &k in &keys {
            view.set(k);
        }
        for &k in &keys {
            assert!(view.test(k), "key {k:#x} should be set");
        }
        // absent keys of the same layer, past the ones written
        for k in layer_sample(16, 4, 1_000_000).into_iter().skip(1) {
            assert!(!view.test(k), "key {k:#x} was never set");
        }
        // taking clears, and reports the transition exactly once
        for &k in &keys {
            assert!(view.take_at(view.index(k)), "first take of {k:#x}");
            assert!(!view.test(k), "{k:#x} survived its take");
            assert!(!view.take_at(view.index(k)), "second take of {k:#x}");
        }
    }

    #[test]
    fn layers_do_not_alias() {
        // the same bit position in two layers must be two different words
        let pool = KeySetPool::new(16);
        let a = pool.layer(15);
        let b = pool.layer(16);
        a.set_at(7);
        assert!(a.test_at(7));
        assert!(!b.test_at(7));
        pool.scratch().set_at(7);
        assert!(!b.test_at(7));
        assert!(pool.scratch().test_at(7));
    }

    #[test]
    fn concurrent_set_loses_no_bits() {
        let keys_per_thread = 5000;
        let nthreads = 8;
        let pool = KeySetPool::new(16);
        let view = pool.layer(16);
        // one dense run of the layer: consecutive keys have consecutive ranks, so
        // many land in the same 64-bit word and race on the same `fetch_or`
        let keys = layer_sample(16, keys_per_thread * nthreads, 1);
        std::thread::scope(|s| {
            for t in 0..nthreads {
                let keys = &keys;
                s.spawn(move || {
                    for k in keys.iter().skip(t).step_by(nthreads) {
                        view.set(*k);
                    }
                });
            }
        });
        for &k in &keys {
            assert!(view.test(k), "key {k:#x} lost under concurrent set()");
        }
        assert_eq!(extracted(view), keys);
    }

    #[test]
    fn concurrent_set_spanning_many_blocks_loses_no_bits() {
        // Spread the keys over many blocks and many summary words, so a dropped
        // summary update makes whole runs vanish from a scan.
        let pool = KeySetPool::new(16);
        let view = pool.layer(16);
        let keys = layer_sample(16, 4000, BLOCK_BITS + 1);
        let blocks: std::collections::HashSet<u64> =
            keys.iter().map(|&k| view.index(k) >> 12).collect();
        assert!(blocks.len() > 100, "want many distinct blocks, got {}", blocks.len());
        std::thread::scope(|s| {
            for t in 0..8 {
                let keys = &keys;
                s.spawn(move || {
                    for k in keys.iter().skip(t).step_by(8) {
                        view.set(*k);
                    }
                });
            }
        });
        let mut expected = keys.clone();
        expected.sort_unstable();
        assert_eq!(extracted(view), expected);
    }

    #[test]
    fn word_bit_set_implies_summary_bit_set() {
        // Deterministic guard on the invariant `set_at`'s conditional summary update
        // depends on, without relying on a race. Covers the first write to a block
        // and a repeat write (which skips the summary RMW).
        let pool = KeySetPool::new(16);
        let view = pool.layer(16);
        let mut keys = layer_sample(16, 8, BLOCK_BITS * 37 + 1);
        // the extremes of the layer, whose indices are 0 and total-1
        keys.push(view.unrank(0, 0).0);
        keys.push(view.unrank(view.capacity() - 1, 0).0);
        for key in keys {
            let block = (view.index(key) >> 6) as usize / BLOCK_WORDS;
            view.set(key);
            assert!(view.test(key), "word bit missing for {key:#x}");
            assert!(view.block_is_marked(block), "summary bit missing for {key:#x}");
            view.set(key);
            assert!(view.test(key), "word bit lost on repeat set of {key:#x}");
            assert!(view.block_is_marked(block), "summary bit lost on repeat set of {key:#x}");
        }
    }

    #[test]
    fn scan_matches_and_is_ordered() {
        let pool = KeySetPool::new(16);
        let view = pool.layer(12);
        // spans many blocks and summary words, plus a pair straddling a block edge,
        // plus the bottom of the layer
        let mut keys = layer_sample(12, 300, CHUNK_WORDS * 64 / 7 + 1);
        keys.extend(layer_sample(12, 2, 1));
        keys.push(view.unrank(0, 0).0);
        for &k in &keys {
            view.set(k);
        }
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(extracted(view), keys);
        assert!(extracted(view).is_sorted(), "rank order must be key order");
        assert_eq!(view.count(), keys.len());
        // and the index a scan reports is the one `index` would give the key
        for (index, key) in view.keys() {
            assert_eq!(index, view.index(key));
        }
    }

    #[test]
    fn par_scan_covers_every_task_boundary() {
        let pool = KeySetPool::new(16);
        let view = pool.layer(14);
        // keys in every block of the first few tasks, including each task's first and
        // last block, so a boundary that is off by one shows up as a missing run
        let mut keys = Vec::new();
        for block in 0..BLOCKS_PER_TASK * 3 + 1 {
            keys.push(view.unrank((block * BLOCK_BITS) as u64, 0).0);
            keys.push(view.unrank((block * BLOCK_BITS + BLOCK_BITS - 1) as u64, 0).0);
        }
        for &k in &keys {
            view.set(k);
        }
        let seen = std::sync::Mutex::new(Vec::new());
        view.par_scan(|task| {
            let got: Vec<u64> = task.map(|(_, key)| key).collect();
            seen.lock().unwrap().extend(got);
        });
        let mut seen = seen.into_inner().unwrap();
        seen.sort_unstable();
        keys.sort_unstable();
        assert_eq!(seen, keys);
    }

    #[test]
    fn drain_scratch_yields_everything_once_and_empties_it() {
        let mut pool = KeySetPool::new(14);
        let view = pool.scratch();
        let mut keys = layer_sample(14, 200, CHUNK_WORDS * 64 / 5 + 1);
        keys.extend(layer_sample(14, 8, 1));
        keys.sort_unstable();
        keys.dedup();
        for &k in &keys {
            view.set(k);
        }
        let drained: Vec<u64> = pool
            .drain_scratch()
            .into_iter()
            .map(|b| b.to_compressed_repr())
            .collect();
        assert_eq!(drained, keys, "drain must yield everything that was set");
        assert!(pool.drain_scratch().is_empty(), "a second drain must find nothing");
        assert_eq!(pool.scratch().count(), 0);
        for k in keys {
            assert!(!pool.scratch().test(k), "key {k:#x} survived the drain");
        }
    }

    #[test]
    fn empty_layer_scans_nothing() {
        let pool = KeySetPool::new(16);
        assert!(extracted(pool.layer(16)).is_empty());
        assert_eq!(pool.layer(16).count(), 0);
    }
}
