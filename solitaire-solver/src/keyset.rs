//! A dense, fixed-size concurrent bitset over `Board`'s compressed key space, used
//! as a replacement for sort+dedup (and, via [`DenseKeySet::test_at`], sorted-merge
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

/// Largest number of keys any single round can produce.
///
/// Two independent constraints cut this down from `2^KEY_BITS`:
///
/// - every board in a BFS round has the same peg count `k`, so only the `C(33, k)` patterns
///   with popcount `k` can occur, and `C(33, 16) = C(33, 17) = 1_166_803_110` is the peak;
/// - every board the solver stores carries [`Board::INVARIANT_TARGET`] (see there), which is
///   four GF(2) conditions and so admits one pattern in 16.
///
/// They are independent - one is a popcount, the other is linear - and compose almost
/// exactly: measured layer by layer, `popcount and invariant` comes to `C(33, k) / 16` to
/// within 0.01% for every `k` from 12 up, peaking at 72_922_839. Summed over all peg counts
/// the invariant admits exactly `2^29` boards, which is the same statement from the other
/// side.
///
/// This is what the bitmap is sized for, indexed by [`DenseKeySet::index`] rather than by the
/// raw key: 8.7 MiB against the 139 MiB the popcount bound alone gives, and a round with fewer
/// pegs uses only a low prefix of it. `examples/hamming_neighbors.rs` derives and verifies both
/// bounds.
///
/// It buys memory, not speed. Measured over interleaved runs of `--repeat 5 calculate-all`,
/// peak RSS falls from 206 MB to 157 MB - the 49 MB is the bitmap's *touched* footprint going
/// from ~58 MB to ~8.7 MB - while the end-to-end time is unchanged within noise (paired
/// medians 104.6/112.7/108.7 ms against 105.0/106.8/107.4 ms, distributions overlapping).
/// Fitting the map in L3 was the hoped-for second win and it did not materialise, which fits
/// what the prefetch ring in `feasible.rs`'s generator already implies: the DRAM latency was
/// being hidden before this, so removing it had nothing left to save.
const MAX_LAYER_KEYS: usize = 72_922_839;
/// words covered by one summary word, i.e. one unit of the bulk operations below.
/// Padding the bitmap up to a multiple of this keeps every chunk they take full,
/// so none of them need a partial-chunk case.
const CHUNK_WORDS: usize = BLOCK_WORDS * 64;
/// one bit per rankable key, padded as above -> ~8.7 MiB.
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
///
/// Re-swept on a verified native build, since the original choice predates the
/// discovery that the dev shell's `RUSTFLAGS` can silently drop `target-cpu`.
/// Raising this halves `high_cum` and doubles `low_rank`, so it trades the two
/// tables against each other rather than shrinking both:
///
/// ```text
///   LOW_BITS   low_rank   high_cum      total   internal (13 reps, order rotated)
///         14     32 KiB   2048 KiB   2080 KiB   72.69 ms   (+2.39)
///         15     64 KiB   1024 KiB   1088 KiB   71.39 ms   (+1.09)
///         16    128 KiB    512 KiB    640 KiB   70.30 ms   <- kept
///         17    256 KiB    256 KiB    512 KiB   70.02 ms   (-0.28)
///         18    512 KiB    128 KiB    640 KiB   69.76 ms   (-0.54)
/// ```
///
/// 14 and 15 are reliably worse: a 1-2 MiB `high_cum` stops fitting the cache the
/// hot ranking path needs it in, and that is the table read on every one of the
/// ~59M keys a run ranks. Above 16 the sweep hints at a small gain, but it does not
/// survive a focused test - 16 against 18 over 25 paired reps came out the *other*
/// way, +1.92 ms median with 18 ahead in only 5 of 25. Two experiments disagreeing
/// on the sign is what no effect looks like when run-to-run drift exceeds the gap,
/// so 16 stays.
///
/// Raising it is not a free constant change in any case: `low_unrank` holds the low
/// half's *values*, which stop fitting `u16` past 16 bits, so it has to widen to
/// `u32` and double. That was implemented to run the sweep and is deliberately not
/// in the tree, since it buys nothing.
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
    high_state: &[u8],
    low_unrank: &[Vec<Vec<u16>>],
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
    // same reasoning as `retarget`: this prefix's keys are exactly those whose low half
    // carries the state the prefix is missing
    let wanted = (high_state[h] ^ Board::INVARIANT_TARGET) as usize;
    let low = low_unrank[pegs - used][wanted][(index - high_cum[h] as u64) as usize];
    let key = ((h as u64) << LOW_BITS) | low as u64;
    debug_assert_eq!(key.count_ones() as usize, pegs);
    debug_assert_eq!(Board::invariant_state(key), Board::INVARIANT_TARGET);
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
/// This trades directly against [`DenseKeySet::set_at`]'s summary update. Coarser
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

/// The bijection between the boards of one layer - the keys of a given peg count that also
/// carry [`Board::INVARIANT_TARGET`] - and `0..layer_keys`, in both directions.
///
/// Its own type rather than four fields on [`DenseKeySet`] because the ranking is a
/// separable concern from the bitmap, and because it is what a caller would need a
/// second instance of to store *ranks* instead of boards - see the note on
/// `feasible.rs`'s `intersect_chunk` for why that was measured and dropped.
pub(crate) struct LayerRanks {
    /// `low_rank[l]` = how many 16-bit values below `l` share both its popcount *and* its
    /// invariant state. Independent of the layer, so built once.
    ///
    /// Narrowing the class from popcount alone to popcount-and-state is the whole of the
    /// invariant saving, and it costs [`LayerRanks::rank`] nothing: the state is a property of
    /// `l`, so it is folded into this table at build time and the hot path is the same two
    /// lookups and an add it always was.
    low_rank: Vec<u16>,
    /// `low_unrank[j][state]` = the 16-bit values with popcount `j` and invariant state
    /// `state`, ascending; the inverse of `low_rank`, needed only by the unranking direction.
    low_unrank: Vec<Vec<Vec<u16>>>,
    /// `low_count[j][state]` = `low_unrank[j][state].len()`, kept separately so
    /// [`Self::retarget`] can read the counts while holding `high_cum` mutably.
    low_count: [[u32; 16]; LOW_BITS as usize + 1],
    /// `high_state[h]` = the invariant state of the prefix `h << LOW_BITS`.
    ///
    /// Precomputed because it does *not* depend on the layer, while both places that want it -
    /// [`Self::retarget`] and [`unindex`] - are per-round or per-key. `Board::invariant_state`
    /// is a loop over the prefix's set bits, up to 17 of them, and `retarget` wants it for all
    /// `HIGH_ENTRIES` prefixes every time the peg count changes: a profile put 8.35% of the
    /// whole run inside `begin_round`, almost all of it on that loop, which is what this
    /// removes. One byte per prefix, 128 KiB, built once.
    high_state: Vec<u8>,
    /// `high_cum[h]` = keys below the prefix `h << LOW_BITS` that have this layer's
    /// peg count. Depends on that count, so rebuilt by [`Self::retarget`].
    high_cum: Vec<u32>,
    /// peg count shared by every key of this layer; the ranking is only a bijection
    /// within one such layer.
    pegs: usize,
    /// how many keys this layer ranks onto, i.e. one past the largest index
    /// [`Self::rank`] can return. Set by [`Self::retarget`].
    layer_keys: u64,
}

impl LayerRanks {
    fn new() -> Self {
        let c = binomials();
        // low_rank / low_unrank are inverses of each other, built in one pass: walking
        // `l` upwards visits the values of each popcount in ascending order, so the
        // running counter per popcount *is* the rank, and the position it is pushed
        // to is that rank.
        let mut low_rank = vec![0u16; 1 << LOW_BITS];
        let mut low_unrank: Vec<Vec<Vec<u16>>> = (0..=LOW_BITS as usize)
            .map(|j| {
                // an even split over the 16 states is the right capacity hint, and is what
                // the counts actually come out at away from the extreme popcounts
                let per_state = (c[LOW_BITS as usize][j] as usize).div_ceil(16);
                (0..16).map(|_| Vec::with_capacity(per_state)).collect()
            })
            .collect();
        for l in 0..(1u32 << LOW_BITS) {
            let j = l.count_ones() as usize;
            let state = Board::invariant_state(l as u64) as usize;
            low_rank[l as usize] = low_unrank[j][state].len() as u16;
            low_unrank[j][state].push(l as u16);
        }
        let mut low_count = [[0u32; 16]; LOW_BITS as usize + 1];
        for (j, states) in low_unrank.iter().enumerate() {
            for (state, values) in states.iter().enumerate() {
                low_count[j][state] = values.len() as u32;
            }
        }
        Self {
            low_rank,
            low_unrank,
            low_count,
            high_state: (0..HIGH_ENTRIES)
                .map(|h| Board::invariant_state((h as u64) << LOW_BITS))
                .collect(),
            high_cum: vec![0u32; HIGH_ENTRIES],
            // no layer yet: `retarget` must run before any key is ranked, and a peg
            // count no board can have makes forgetting it fail the assertions in
            // `rank` rather than silently mis-rank.
            pegs: usize::MAX,
            layer_keys: 0,
        }
    }

    /// Switches to the layer of keys with `pegs` pegs.
    pub(crate) fn retarget(&mut self, pegs: usize) {
        let c = binomials();
        assert!(pegs <= KEY_BITS, "a board cannot hold {pegs} pegs");
        // high_cum[h] = keys with this peg count below prefix h. A prefix that has
        // already used more than `pegs` bits, or too few to be completed by the low
        // half, contributes nothing.
        let counts = self.low_count;
        // borrowed apart from `high_cum`, which is held mutably below
        let high_state = &self.high_state;
        let mut acc = 0u64;
        for (h, slot) in self.high_cum.iter_mut().enumerate() {
            *slot = acc as u32;
            let used = (h as u64).count_ones() as usize;
            if used <= pegs && pegs - used <= LOW_BITS as usize {
                // Only the low halves that complete this prefix *to the target* count: the
                // state is a XOR, so the low half must carry whatever the prefix is missing.
                let wanted = high_state[h] ^ Board::INVARIANT_TARGET;
                acc += u64::from(counts[pegs - used][wanted as usize]);
            }
        }
        debug_assert!(
            acc <= MAX_LAYER_KEYS as u64,
            "layer {pegs} ranks up to {acc}, past the {MAX_LAYER_KEYS} the bitmap is sized for"
        );
        debug_assert!(
            acc <= c[KEY_BITS][pegs],
            "layer {pegs} totals {acc}, more than the C(33, {pegs}) keys of that popcount"
        );
        self.pegs = pegs;
        self.layer_keys = acc;
    }

    /// How many keys the current layer ranks onto - the bound [`DenseKeySet::begin_round`]
    /// checks the bitmap against.
    pub(crate) fn layer_keys(&self) -> u64 {
        self.layer_keys
    }

    /// Position of `key` within its layer: its rank among the keys of the same
    /// popcount, which is dense in `0..C(33, pegs)` where the raw key is spread over
    /// `2^33`. See [`LOW_BITS`] for the two-table form.
    #[inline]
    pub(crate) fn rank(&self, key: u64) -> u64 {
        debug_assert!(key < 1 << KEY_BITS, "key {key:#x} is wider than the board");
        debug_assert_eq!(
            key.count_ones() as usize,
            self.pegs,
            "key {key:#x} is not from this layer - its rank would collide with \
             another board's"
        );
        debug_assert_eq!(
            Board::invariant_state(key),
            Board::INVARIANT_TARGET,
            "key {key:#x} is outside the invariant subspace the ranking is a bijection on, \
             so its rank would collide with another board's"
        );
        self.high_cum[(key >> LOW_BITS) as usize] as u64
            + self.low_rank[(key & LOW_MASK) as usize] as u64
    }

    /// Inverse of [`Self::rank`]; see [`unindex`] for what `hint` is for.
    #[inline]
    pub(crate) fn unrank(&self, index: u64, hint: usize) -> (u64, usize) {
        unindex(
            &self.high_cum,
            &self.high_state,
            &self.low_unrank,
            self.pegs,
            index,
            hint,
        )
    }

    /// The `hint` to start [`Self::unrank`] from when the first index to be decoded is
    /// `first_index` - the one search that lets a run of ascending indices walk
    /// `high_cum` forward instead of searching it per key.
    pub(crate) fn cursor_at(&self, first_index: u64) -> usize {
        self.high_cum
            .partition_point(|&c| c as u64 <= first_index)
            .saturating_sub(1)
    }
}

pub(crate) struct DenseKeySet {
    words: Vec<AtomicU64>,
    summary: Vec<AtomicU64>,
    /// the layer the map is currently holding; see [`Self::begin_round`].
    ranks: LayerRanks,
}

impl DenseKeySet {
    pub(crate) fn new() -> Self {
        let words = zeroed_atomic_vec(NUM_WORDS);
        // Deliberately NOT prefaulted, though the reason has changed and is now weaker than
        // it was. Populating this up front with `MADV_POPULATE_WRITE`, issued from parallel
        // chunks so as not to serialize what the lazy faults do across all workers, measured
        // *worse* by 38 ms (+25.8%, slower in 14 of 14 interleaved reps).
        //
        // That was measured against a 1 GiB mapping indexed by the raw key, of which only a
        // quarter was ever touched - peak RSS 261 MiB, because `normalize` returns the minimum
        // of each board's 8-symmetry orbit and a minimum-of-8 leaves the high bits of the
        // compressed key clear far more often than not, so the keys crowded into the low
        // quarter of the range. Prefaulting therefore committed and zeroed ~760 MiB nothing
        // would ever read, which was the whole of the regression.
        //
        // None of that still holds. Ranking within the layer and then within the invariant
        // subspace (see `MAX_LAYER_KEYS`) has taken the mapping to ~8.7 MiB, essentially all
        // of which a round touches, so the waste the regression consisted of is gone - and so
        // is most of the fault cost that made prefaulting tempting in the first place, since
        // first touch now covers ~8.7 MiB rather than ~58 MiB. Whether prefaulting would now
        // win is genuinely open; it has not been re-measured, and the old number should not be
        // read as saying no.
        Self {
            words,
            summary: zeroed_atomic_vec(SUMMARY_WORDS),
            ranks: LayerRanks::new(),
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
        // Bounded against what the layer actually ranks onto rather than against
        // `C(33, pegs)`: the ranking is a bijection onto the popcount-`pegs` keys that also
        // carry `Board::INVARIANT_TARGET`, which is a sixteenth of them, and sizing the map
        // for the larger figure would waste 16x the memory this exists to save. `retarget`
        // computes the real total as it builds `high_cum`, so ask it afterwards.
        self.ranks.retarget(pegs);
        let needed = self.ranks.layer_keys();
        assert!(
            needed as usize <= NUM_WORDS * 64,
            "layer of {pegs} pegs needs {needed} bits, map holds {}",
            NUM_WORDS * 64
        );
    }

    /// Position of `key` in this round's layer: its rank among the keys of the same
    /// popcount, which is dense in `0..C(33, pegs)` where the raw key is spread over
    /// `2^33`. See [`LOW_BITS`] for the two-table form.
    ///
    /// `pub(crate)` so a caller that both prefetches and then acts on the same key
    /// can compute this once and pass the result to the `*_at` methods, rather than
    /// having each of them re-derive it. That duplication is not free: the two table
    /// reads and their bounds checks profile at ~5.9% of a run when done twice.
    #[inline]
    pub(crate) fn index(&self, key: u64) -> u64 {
        self.ranks.rank(key)
    }

    /// Inverse of [`Self::index`], as a free function so that
    /// [`Self::drain_sorted_by_key`] - which has the fields borrowed apart - and
    /// the tests share one implementation. See [`unindex`].
    #[cfg(test)]
    fn unindex_key(&self, index: u64, hint: usize) -> (u64, usize) {
        self.ranks.unrank(index, hint)
    }

    /// Marks `key` present. Safe to call from many threads at once, but must never
    /// overlap [`DenseKeySet::clear`] - callers always join the filling parallel
    /// region first (see `feasible.rs`, where `clear()` is its own joined region
    /// that runs before generation starts).
    ///
    /// INVARIANT, relied on by [`Self::clear`] and [`Self::drain_sorted_by_key`],
    /// both of which skip whole zero summary
    /// words: if a bit in `words` is set then that bit's block is marked in
    /// `summary`. Every call reaches the summary code below, so each setter either
    /// marks the block itself or observes it already marked - which, by induction,
    /// means some call did mark it. Readers all run after the generating rayon
    /// region has joined, and that join supplies the happens-before edge making
    /// these `Relaxed` writes visible; that is what the surrounding code already
    /// relied on.
    /// Convenience for tests, which think in keys; every hot path ranks the key
    /// itself and calls [`Self::set_at`] so the rank is computed once.
    #[cfg(test)]
    pub(crate) fn set(&self, key: u64) {
        self.set_at(self.index(key));
    }

    /// Marks the bit at a position already obtained from [`Self::index`]; the
    /// concurrency contract is the same as `set`'s below.
    #[inline]
    pub(crate) fn set_at(&self, bit: u64) {
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
        //
        // And it is not worth dropping the atomic on the single-threaded path, though
        // a profile makes it look like the obvious win: at `--threads 1` this
        // function is 43.6% of the run against 20.1% at 16 threads, the same
        // instruction apparently costing twice the share for want of other cores'
        // misses to hide behind. Tried it - a serial generator taking the map by
        // `&mut` (which *proves* exclusivity, where a `threads == 1` test would not:
        // `par::configure_thread_pool` leaves an already-built pool alone, so that
        // count does not bound how many rayon workers exist) and writing plain
        // `u64`s. Measured at `--threads 1`: generation +0.4%, total +4.3 ms, faster
        // in 6 of 12 paired runs. No effect.
        //
        // Because the share is the memory access, not the lock. `prefetch_at` issues
        // the fetch 16 keys ahead, so the line is generally resident by the time the
        // RMW runs, and an uncontended `lock or` on a resident line costs about what
        // a plain read-modify-write costs on this core. Reverted: `unsafe` plus a
        // duplicated generation loop, for nothing.
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

    /// Starts fetching the cache line [`Self::set_at`]/[`Self::test_at`] would
    /// touch for `bit`, without waiting for it. Takes a position from
    /// [`Self::index`] so a caller doing both does not rank the key twice.
    ///
    /// It earns its keep on the *write* side because a `lock`ed RMW is a full
    /// barrier: it drains the store buffer, so consecutive independent misses
    /// cannot overlap in the core's line-fill buffers and each pays the full
    /// latency in series. Guarding the `fetch_or` with a load does not fix that (it
    /// is dependent, and touches the same line) - which is why that guard lost
    /// while this wins; issuing the fetch some distance ahead of the RMW restores
    /// the memory-level parallelism the `lock` prefix destroys.
    ///
    /// On the *read* side the argument is different, since `test_at` is a plain
    /// `Relaxed` load and no barrier at all - the core will overlap probes on its
    /// own. What limits it there is how far ahead the out-of-order window can run,
    /// and `intersect_chunk` puts a thoroughly unpredictable data-dependent branch
    /// on every probe, so the window keeps being spent on mispredicted work instead
    /// of starting the next miss. Both callers pipeline this against their own work
    /// via a ring buffer; see them for the distance.
    ///
    /// `_MM_HINT_ET0` would be the natural hint for the write side (write intent ->
    /// line arrives Modified, so the RMW needs no second round trip for ownership),
    /// but LLVM's x86 backend lowers it to a plain `prefetcht0` anyway. That is
    /// close enough here: nothing else is writing these lines concurrently in the
    /// common case, so they arrive Exclusive and the RMW can upgrade locally.
    #[inline]
    pub(crate) fn prefetch_at(&self, bit: u64) {
        #[cfg(target_arch = "x86_64")]
        {
            // `index` maps into this round's layer, whose size `begin_round` has
            // checked against the map - the same bound `set`/`test` index under.
            let word_idx = (bit >> 6) as usize;
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
        let _ = bit;
    }

    /// Convenience for tests; see [`Self::set`].
    #[cfg(test)]
    pub(crate) fn test(&self, key: u64) -> bool {
        self.test_at(self.index(key))
    }

    /// Tests the bit at a position already obtained from [`Self::index`].
    #[inline]
    pub(crate) fn test_at(&self, bit: u64) -> bool {
        let word_idx = (bit >> 6) as usize;
        (self.words[word_idx].load(Ordering::Relaxed) >> (bit & 63)) & 1 != 0
    }

    /// Reinterprets an exclusively-borrowed atomic slice as plain `u64`s.
    ///
    /// Atomics are only needed while [`Self::set_at`] is running concurrently. The
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
    ///
    /// *Drains*: the map is empty afterwards, because each block is zeroed as soon as
    /// it has been read. That is nearly free here and is not free in `clear`. This
    /// already reads every touched word, so the line is in L1 when the zero store
    /// lands; `clear` running later has to fetch each line back from DRAM first, only
    /// to overwrite it. Both pay the same writeback, so what fusing removes is the
    /// re-read - measured at 83.5 MiB of the 201 MiB a run's clears move, since only
    /// rounds that extract can donate their reads this way (the shrink phase probes
    /// instead, and never walks the map).
    ///
    /// `begin_round` still calls `clear`, which is now a cheap summary scan for any
    /// round following an extraction. Keeping that call rather than relying on this
    /// one is deliberate: it is what makes a stale layer impossible if some future
    /// path stops extracting.
    ///
    /// Only boards satisfying `keep` are emitted. Taking the predicate here rather
    /// than leaving the caller to filter afterwards saves a whole pass over the
    /// result and, more to the point, the buffer that pass would write into: the
    /// caller's alternative is `par::par_filter`, which allocates and copies the
    /// full set a second time. It also evaluates `keep` once per board where
    /// `par_filter` evaluates it twice, since that has to count survivors before it
    /// can size its output.
    ///
    /// This does mean the per-chunk `Vec`s are sized by the bit count rather than
    /// the survivor count, so they over-allocate by whatever `keep` rejects (up to
    /// ~16% for the pagoda pruning this exists for). That is the cheaper side of the
    /// trade - they are short-lived and `par::par_join` copies out only what was
    /// filled - and it is exactly the double evaluation being avoided.
    pub(crate) fn drain_sorted_by_key(
        &mut self,
        keep: impl Fn(Board) -> bool + Send + Sync,
    ) -> Vec<Board> {
        // `&mut` lets this read plain `u64`s rather than `Relaxed` atomics; see
        // `as_plain`. It matters most for the counting pass below, which is a
        // popcount over every word of every touched block - vectorizable as plain
        // loads, not as atomic ones.
        let Self {
            words,
            summary,
            ranks,
        } = self;
        let words: &mut [u64] = Self::as_plain(words);
        let summary: &mut [u64] = Self::as_plain(summary);
        // One chunk per summary word, as `clear` takes them, so each worker owns the
        // words its summary word covers and can zero them.
        let chunks: Vec<Vec<Board>> = words
            .par_chunks_mut(CHUNK_WORDS)
            .zip(summary.par_iter_mut())
            .enumerate()
            .map(|(sword_idx, (chunk, sword))| {
                let bits = std::mem::replace(sword, 0);
                if bits == 0 {
                    return Vec::new();
                }
                // count first so this chunk's `Vec` is allocated exactly once,
                // without growing it by repeated reallocation - one of many (one per
                // set summary word) small per-chunk allocations that `par_join`
                // below has to copy out of again. This counts set bits, not
                // survivors of `keep`, so it is an upper bound; see the doc comment.
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
                // Set bits are positions in this layer's ranking, so each one has to
                // be turned back into a key. Indices within a chunk only ever
                // increase and `high_cum` is monotone, so instead of searching it per
                // key a cursor walks forward - O(1) amortised, with one search to
                // find where this chunk starts. Total cursor travel across all chunks
                // is bounded by `high_cum`'s length, not by the number of keys.
                let first_index = (sword_idx * CHUNK_WORDS * 64) as u64;
                let mut h = ranks.cursor_at(first_index);
                let mut b = bits;
                while b != 0 {
                    let block_in_chunk = b.trailing_zeros() as usize * BLOCK_WORDS;
                    let block = sword_idx * 64 + b.trailing_zeros() as usize;
                    for (wi, w) in chunk[block_in_chunk..block_in_chunk + BLOCK_WORDS]
                        .iter_mut()
                        .enumerate()
                    {
                        let mut wbits = *w;
                        // drained: zeroing here rather than leaving it to the next
                        // round's `clear` is most of the point of this method - see
                        // the doc comment
                        *w = 0;
                        while wbits != 0 {
                            let bit = wbits.trailing_zeros();
                            let word_idx = block * BLOCK_WORDS + wi;
                            let index = ((word_idx as u64) << 6) | bit as u64;
                            let key;
                            (key, h) = ranks.unrank(index, h);
                            let board = Board::from_compressed_repr(key);
                            if keep(board) {
                                out.push(board);
                            }
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
        // core doing a large memmove while the other 15 sat idle. `_owned` so that a
        // round whose bits all land in one summary chunk skips the copy entirely.
        crate::par::par_join_owned(chunks)
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
    /// alone, and `rank` rejects the rest, so a test feeding it arbitrary popcount-`pegs`
    /// patterns would be exercising an input the solver cannot produce - which is exactly
    /// what these helpers used to do before the invariant narrowed the layer.
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
    /// The independent reference for [`LayerRanks::rank`]'s two-table form, and the successor
    /// to `colex_rank`, which counted among all popcount-`pegs` keys and so no longer
    /// describes what `index` returns. Walks the bits from the top; at each set bit, every
    /// smaller key agreeing above it has a zero there, so it counts the completions below that
    /// carry whatever state the prefix still owes the target.
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
    /// No longer what `index` returns - the ranking is a bijection onto the *invariant*
    /// subspace now, and `subspace_rank` is its reference. Kept because the two are related in
    /// a way worth pinning: restricting to a subset of the keys can only pull ranks down, so
    /// the subspace rank must never exceed the popcount rank.
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
        // 16 and 17 are the peak layers before the invariant is taken into account
        assert_eq!(c[KEY_BITS][16], 1_166_803_110);
        assert_eq!(c[KEY_BITS][17], 1_166_803_110);
        // and `MAX_LAYER_KEYS` is now a sixteenth of that rather than that, because the
        // ranking is a bijection onto the invariant subspace - `layer_sizes_fit_the_map`
        // pins it against what `retarget` actually produces
        assert!(
            (MAX_LAYER_KEYS as u64) < c[KEY_BITS][16],
            "the invariant must shrink the peak layer, not grow it"
        );
        // and the row must account for the whole key space
        assert_eq!(
            (0..=KEY_BITS).map(|k| c[KEY_BITS][k]).sum::<u64>(),
            1u64 << KEY_BITS
        );
    }

    /// Every layer has to fit the bitmap, and `MAX_LAYER_KEYS` has to be the tightest bound
    /// on that - too small silently truncates a layer, too large wastes the memory the
    /// invariant exists to save.
    #[test]
    fn map_holds_every_layer() {
        let ways = ways_below();
        let target = Board::INVARIANT_TARGET as usize;
        let mut peak = 0u64;
        // `ways[KEY_BITS]` has one row per peg count, so enumerating it covers 0..=KEY_BITS
        for (pegs, states) in ways[KEY_BITS].iter().enumerate() {
            let keys = states[target];
            assert!(
                keys <= (NUM_WORDS * 64) as u64,
                "layer of {pegs} pegs needs {keys} bits, map holds {}",
                NUM_WORDS * 64
            );
            peak = peak.max(keys);
        }
        assert_eq!(
            peak, MAX_LAYER_KEYS as u64,
            "MAX_LAYER_KEYS must be exactly the peak layer"
        );
        // and the layers must partition the invariant subspace: one key in 16, i.e. 2^29
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
        // distinct index, and together they must cover 0..C(33, pegs) exactly. A
        // collision here would silently merge two boards in the map.
        let ways = ways_below();
        for pegs in [1usize, 2, 3] {
            let mut set = DenseKeySet::new();
            set.begin_round(pegs);
            let keys = layer_keys(pegs);
            // the layer is the popcount-`pegs` keys carrying the target invariant, which the
            // independent DP counts the same way `retarget` accumulates it
            assert_eq!(
                keys.len() as u64,
                ways[KEY_BITS][pegs][Board::INVARIANT_TARGET as usize],
                "enumeration and the DP disagree on the size of layer {pegs}"
            );
            assert!(!keys.is_empty(), "layer {pegs} came out empty");
            let mut seen: Vec<u64> = keys.iter().map(|&k| set.index(k)).collect();
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
        for pegs in [1usize, 2, 5, 16, 17, 32, 33] {
            let mut set = DenseKeySet::new();
            set.begin_round(pegs);
            let total = ways[KEY_BITS][pegs][Board::INVARIANT_TARGET as usize];
            for key in layer_sample(pegs, 500, 977) {
                let index = set.index(key);
                assert_eq!(
                    index,
                    subspace_rank(key, pegs, &ways),
                    "pegs={pegs} key={key:#x}"
                );
                assert!(index < total);
                // dropping 15/16 of the keys can only pull a rank down, never up
                assert!(
                    index <= colex_rank(key, &binomials()),
                    "subspace rank exceeds the popcount rank for {key:#x}"
                );
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
        assert_eq!(set.drain_sorted_by_key(|_| true).len(), keys.len());
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
        let blocks: std::collections::HashSet<u64> =
            keys.iter().map(|&k| set.index(k) >> 12).collect();
        assert!(
            blocks.len() > 100,
            "want many distinct blocks, got {}",
            blocks.len()
        );
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
            .drain_sorted_by_key(|_| true)
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
        // Include the extremes of the layer, whose indices are 0 and total-1. Taken through
        // `unindex_key` rather than built as the lowest and highest popcount-`pegs` patterns:
        // those are no longer in the layer, since the ranking now covers only the keys
        // carrying the target invariant, and walking Gosper's hack to find the real top of a
        // 72M-key layer is not something a test should do.
        let total = set.ranks.layer_keys();
        keys.push(set.unindex_key(0, 0).0);
        keys.push(set.unindex_key(total - 1, 0).0);
        for key in keys {
            set.set(key);
            assert!(set.test(key), "word bit missing for {key:#x}");
            assert!(summary_bit_set(key), "summary bit missing for {key:#x}");
            set.set(key);
            assert!(set.test(key), "word bit lost on repeat set of {key:#x}");
            assert!(
                summary_bit_set(key),
                "summary bit lost on repeat set of {key:#x}"
            );
        }
    }

    #[test]
    fn drain_sorted_by_key_matches_and_is_ordered() {
        let pegs = 12;
        let mut set = DenseKeySet::new();
        set.begin_round(pegs);
        // spans many blocks and summary words, plus a pair straddling a block edge
        let mut keys = layer_sample(pegs, 300, CHUNK_WORDS * 64 / 7 + 1);
        let straddle = layer_sample(pegs, 2, 1);
        keys.extend(straddle);
        // the bottom of the layer, i.e. index 0 - see the note in
        // `word_bit_set_implies_summary_bit_set` on why this is not `(1 << pegs) - 1`
        keys.push(set.unindex_key(0, 0).0);
        for &k in &keys {
            set.set(k);
        }
        let extracted: Vec<u64> = set
            .drain_sorted_by_key(|_| true)
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
        // deliberately NOT drained first - draining would empty the map and leave the
        // assertion below passing for the wrong reason
        assert!(old.iter().all(|&k| set.test(k)));

        set.begin_round(15);
        assert!(
            set.drain_sorted_by_key(|_| true).is_empty(),
            "the previous layer's bits survived a begin_round"
        );
        let new = layer_sample(15, 10, 5);
        for &k in &new {
            set.set(k);
        }
        let extracted: Vec<u64> = set
            .drain_sorted_by_key(|_| true)
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
        // checked without draining, so that `clear` is what empties the map here
        assert!(keys.iter().all(|&k| set.test(k)));
        set.clear();
        assert!(set.drain_sorted_by_key(|_| true).is_empty());
        for k in keys {
            assert!(!set.test(k));
        }
    }

    /// The drain is what lets `feasible.rs` skip a `clear` pass, so an incomplete
    /// one would leave a stale layer for the next round to misread as its own keys.
    #[test]
    fn drain_leaves_the_map_empty() {
        let pegs = 14;
        let mut set = DenseKeySet::new();
        set.begin_round(pegs);
        // spans many blocks and summary words, plus a dense run inside one block
        let mut keys = layer_sample(pegs, 200, CHUNK_WORDS * 64 / 5 + 1);
        keys.extend(layer_sample(pegs, 8, 1));
        keys.sort_unstable();
        keys.dedup();
        for &k in &keys {
            set.set(k);
        }
        let drained: Vec<u64> = set
            .drain_sorted_by_key(|_| true)
            .into_iter()
            .map(|b| b.to_compressed_repr())
            .collect();
        assert_eq!(drained, keys, "drain must yield everything that was set");
        assert!(
            set.drain_sorted_by_key(|_| true).is_empty(),
            "a second drain must find nothing"
        );
        for k in keys {
            assert!(!set.test(k), "key {k:#x} survived the drain");
        }
        // the summary has to be cleared too, or a later drain would skip live blocks
        assert!(
            set.summary.iter().all(|w| w.load(Ordering::Relaxed) == 0),
            "summary not cleared by the drain"
        );
    }

    #[test]
    fn empty_set_extracts_nothing() {
        let mut set = DenseKeySet::new();
        set.begin_round(16);
        assert!(set.drain_sorted_by_key(|_| true).is_empty());
    }
}
