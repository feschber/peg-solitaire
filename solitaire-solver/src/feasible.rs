use std::time::Duration;

use std::num::NonZero;

use log::info;

// the dense-bitset path (see keyset.rs) is off on wasm32 (its flat 139 MiB
// allocation is still a poor citizen in a browser) and, for now, on Android too:
// solitaire-game calls calculate_feasible_set on startup on a real,
// potentially memory-constrained device that hasn't been tested against this
// allocation - revisit once that's been measured on a representative device.
// Worth revisiting: that allocation used to be 1 GiB, and ranking the key space
// (see keyset.rs) brought it down to 139 MiB, of which ~167 MiB peak resident
// across the whole process - which makes the Android case a lot more plausible
// than when this was written.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
use rayon::prelude::*;

use crate::{
    Board, Dir,
    keyset::DenseKeySet,
    par::{self, ParDedup},
    sort::Sort,
    timer::Timer,
};

/// boards-in-round threshold above which a round uses the dense bitset (see
/// `keyset.rs`) instead of sort+dedup+merge-intersect.
///
/// This has walked down twice, each time because the bitset got cheaper. It was
/// 1_000_000 - capturing only the single biggest round of each phase - until
/// `generate_into_bitset` got its prefetch pipeline, which moved it to 50_000:
///
/// ```text
/// 1_000_000    203.9 ms        50_000    181.3 ms
///   400_000    191.7 ms        25_000    182.4 ms
///   200_000    183.5 ms        10_000    179.3 ms
///   100_000    182.6 ms         2_000    182.3 ms
///                                  100    185.7 ms
/// ```
///
/// Ranking the key space then shrank the map 7.4x and the summary index with it
/// (256 KiB -> 35 KiB), which is precisely the fixed per-round cost that used to
/// penalize small rounds - `clear` and extraction both scan the whole summary. So
/// it was re-swept, 9-11 interleaved reps per point, internal timer medians:
///
/// ```text
///   200_000    100.1 ms         3_000     83.7 ms
///    50_000     88.5 ms         2_000     84.4 ms
///    20_000     87.6 ms         1_000     84.9 ms
///    10_000     85.5 ms           500     84.7 ms
///     5_000     85.1 ms             1     87.2 ms
/// ```
///
/// Now flat from roughly 500 to 10_000 - a 1.9 ms spread across that whole band,
/// i.e. inside the noise - so 2_000 is picked as the middle of it rather than the
/// measured minimum of 3_000, which is not meaningfully better than its neighbours.
///
/// Worth ~5% against 50_000, give or take: the sweep above puts the two 4.1 ms
/// apart, and a dedicated paired run (14 interleaved reps of just those two values)
/// gave -6.3 ms on medians but only -1.5 ms on minima, faster in 14 of 14. The
/// spread between those framings is the honest uncertainty - the lower threshold
/// also has visibly tighter run-to-run variance, which flatters a median
/// comparison, so treat ~4 ms as the conservative read.
///
/// Note the two ends both turn up, for opposite reasons, so this is a real optimum
/// and not a monotone preference: at 200_000 the mid-size rounds are back on the
/// sort path that the bitset now beats handily, while at 1 - every round on the
/// bitset - the per-round costs that do *not* scale with the round (the summary
/// scans, rayon dispatch, `begin_round`'s prefix sum) stop being amortized. The
/// sort path still earns its keep for the smallest rounds.
///
/// `begin_round`'s table rebuild was the obvious suspect for that lower turn-up,
/// since ranking added it, but it profiles at 0.09% of the run - the summary scans
/// and rayon dispatch dominate the fixed cost instead.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
const BITSET_THRESHOLD_DEFAULT: usize = 2_000;

/// [`BITSET_THRESHOLD_DEFAULT`], overridable for tuning sweeps as the `peeka_sort`
/// knobs are. Read once; it only gates a per-round size comparison, so reading it
/// from the environment cannot change any inner loop's codegen - a sweep measures
/// exactly what hardcoding the value would do.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn bitset_threshold() -> usize {
    use std::sync::OnceLock;
    static T: OnceLock<usize> = OnceLock::new();
    *T.get_or_init(|| {
        std::env::var("PEG_BITSET_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(BITSET_THRESHOLD_DEFAULT)
    })
}

/// pagoda-function pruning for the growth phase (see `pagoda.rs`): a board whose
/// inverse cannot reach the solved position is unreachable from it too, so it can
/// never contribute.
///
/// A `fn` rather than a closure so both round paths can share it without either
/// having to thread a capture around.
///
/// Stated without materializing the inverse: every valid cell is occupied in the
/// full board and invalid cells weigh nothing, so `pagoda(b.inverse())` is just
/// `FULL_WEIGHT - pagoda(b)` (asserted by a test in `pagoda.rs`). Both weights are
/// `const`, which also retires the `OnceLock` that used to hold `pagoda(solved)`.
///
/// A simplification, not a speedup - measured neutral (7 of 15 interleaved reps,
/// medians within noise) even though it runs ~5.1M times a run. The `OnceLock` was
/// evidently already being hoisted out of the filter loops.
fn growth_survives_pagoda(board: Board) -> bool {
    crate::pagoda::FULL_WEIGHT - crate::pagoda::pagoda(board) >= crate::pagoda::SOLVED_WEIGHT
}

/// generates every forward (or, if `!forward`, reverse) move from `states`,
/// normalizes, and sets each result's compressed key directly in `set` - fusing
/// generation + normalize + dedup into a single pass with no intermediate `Vec`.
///
/// deliberately does NOT apply pagoda-function pruning (see `pagoda.rs`) here:
/// that was tried (both unconditionally and gated behind `set.test()` to skip
/// already-set keys) and measurably regressed wall time despite shrinking the
/// deduped/extracted output. The cost of checking pagoda scales with the raw
/// move count (millions per round here), while its benefit scales with the
/// distinct result count (usually a small fraction of that) - paying the check
/// at the wrong granularity outweighed what it saved. Callers apply pagoda to
/// the deduped/extracted result instead, where the cost/benefit ratio is right.
///
/// That reasoning has since been tested against its own strongest counter-argument
/// and survived, which is worth recording because it kills the whole idea rather
/// than one instance of it. The objection above is a cost objection - the check is
/// too expensive per move - and it is removable: a move is an XOR with a fixed
/// three-cell mask and pagoda is a sum over occupied cells, so a move's effect on
/// the weight is a *constant of the move*. Tabulating it (`pagoda::MOVE_DELTA` as
/// was) turns the test into one L1 read and an add against a weight computed once
/// per source board, evaluated *before* `normalize_after_move`'s eight symmetries,
/// the `pext`, the rank lookups and the `lock or` - so a pruned move costs almost
/// nothing and skips everything. It prunes a real fraction of the stream:
///
/// ```text
///   shrink round 17   2,613,363 of 19,672,499 moves   13.3%
///   largest growth    1,781,496 of 14,274,701 moves   12.5%
///   whole run         ~6,200,000 of 59,193,176        10.5%
/// ```
///
/// It is still a loss: +1.58 ms on the internal timer (+2.2%), +1.86 ms wall,
/// faster in only 3 of 15 interleaved reps. Per round, *every* round of any size
/// came out flat or worse - including the 13.3% one, at +0.18 ms - so there is no
/// subset worth gating it to either.
///
/// The mechanism is the same one that sank the recently-seen filter documented
/// below, and it is really a statement about this loop: the `lock or`s are already
/// latency-hidden by the prefetch pipeline, so removing a tenth of them recovers
/// almost nothing, while a test on the critical path is paid by all ten tenths.
/// Anything that filters the move stream in flight has to beat that, and the
/// cheapest possible such filter does not.
///
/// It also cannot avoid generating the duplicates in the first place, which is
/// worth recording because the idea keeps looking plausible. Measured on the two
/// biggest rounds:
///
/// ```text
///                          growth round   shrink round
///   moves                    14,274,701     19,672,499
///   distinct un-normalized    5,121,307      6,129,021    (2.79x / 3.21x)
///   distinct normalized       2,499,905      3,163,355    (5.71x / 6.22x total)
///   intra-board duplicates        5,592          7,077    (0.04% of moves)
/// ```
///
/// So the ~6x splits into genuine graph convergence (2.8-3.2x: different
/// predecessors reaching the identical board) and symmetry collapse (~2x:
/// different members of one orbit normalizing together). Neither is decidable
/// from `(board, move)` alone. A result `R` is generated once per forward move
/// of `R` whose predecessor happens to be solvable, so choosing a single
/// canonical generator means asking which of `R`'s ~10 predecessors are
/// solvable: ~10 normalizations plus ~10 random probes into a 1 GiB structure,
/// to skip ~6 prefetched `lock or`s. The only rule that *is* local, "emit only
/// if this is `R`'s lowest-index forward move", silently drops every `R` whose
/// lowest-index predecessor is unsolvable.
///
/// The one locally-removable class - two moves from the same board related by
/// that board's symmetry stabilizer - is 0.04% of moves (only ~0.1% of boards
/// have a nontrivial stabilizer at all), so it is not worth a branch.
///
/// Returns the total number of moves considered (pre-dedup).
///
/// Generation and the bitset writes are software-pipelined `PREFETCH_DISTANCE`
/// keys apart, via the ring buffer below: each key is prefetched (see
/// [`DenseKeySet::prefetch_at`]) as soon as it is computed, and only
/// `set()` once that many further keys have been generated. Profiling the
/// straight-line version put ~70% of this function's cycles on the single
/// `lock or` inside `set` - a serialized DRAM round trip per call, because the
/// `lock` prefix bars the overlapping misses that would otherwise hide it. The
/// ~8 symmetries `normalize` evaluates per key are exactly the independent work
/// needed to cover that latency.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn generate_into_bitset(states: &[Board], set: &DenseKeySet, forward: bool) -> usize {
    /// how many keys ahead of the `set()` the prefetch runs. Must be a power of two
    /// (the ring index is masked, not divided).
    const PREFETCH_DISTANCE: usize = 16;

    /// boards per work unit. The ring has to persist across boards to stay full -
    /// these rounds average only ~10 moves per board, well under the distance - so
    /// the parallel split is per chunk rather than rayon's default per element.
    /// Small enough that a chunk is still a fraction of one thread's share (these
    /// rounds run 1.4-2.0M boards over 16 threads), keeping work-stealing effective.
    const CHUNK: usize = 2048;

    // Deliberately no recently-seen filter in front of `set()`. ~85% of the keys
    // here are repeats, so a small L1-resident direct-mapped tag table (index on
    // `key & (SLOTS - 1)`, tag the remaining bits, skip on a match - exact, so it
    // cannot drop a key) looks like it should erase most of the remaining DRAM
    // traffic for an L1 access. Measured, and it does not: the duplicates' reuse
    // distance is far longer than any table that stays in cache.
    //
    //     slots   keys reaching set(), growth round   shrink round
    //      none            14,274,701  (100%)         19,672,499  (100%)
    //      4096            11,850,941  ( 83%)         16,602,185  ( 84%)
    //     16384            11,302,245  ( 79%)         15,908,615  ( 81%)
    //
    // So it removes under a fifth of the calls while charging a load, a store and
    // a poorly-predicted branch against *all* of them - and the branch breaks up
    // the prefetch pipeline below, which is worth far more. Net +5.4% on the
    // generation step, +5.9% end to end. Tried at both sizes, then reverted.

    states
        .par_chunks(CHUNK)
        .map(|chunk| {
            let mut ring = [0u64; PREFETCH_DISTANCE];
            // also the ring index: every increment corresponds to a slot written,
            // which is what makes the drain below correct.
            let mut n = 0usize;
            for board in chunk {
                // hoisted out of the move loop: a move is an XOR with a constant
                // mask and the symmetry transforms are GF(2)-linear, so every
                // successor's eight symmetries are these eight XORed against a
                // per-move constant. See `Board::normalize_after_move`.
                let syms = board.symmetries();
                for dir in Dir::enumerate() {
                    let mask = if forward {
                        board.mov_pattern_mask(dir)
                    } else {
                        board.rev_mov_pattern_mask(dir)
                    };
                    for idx in mask {
                        let moved = Board::normalize_after_move(&syms, idx, dir);
                        // the ring holds bit positions rather than keys so that the
                        // rank is computed once per move: the prefetch and the `set`
                        // both need it, and having each derive it from the key made
                        // `index` and its two bounds-checked table reads ~5.9% of a
                        // run instead of ~3%.
                        let bit = set.index(moved.to_compressed_repr());
                        set.prefetch_at(bit);
                        let slot = n & (PREFETCH_DISTANCE - 1);
                        // once the ring is full, the slot about to be overwritten
                        // holds the bit prefetched exactly PREFETCH_DISTANCE moves ago
                        if n >= PREFETCH_DISTANCE {
                            set.set_at(ring[slot]);
                        }
                        ring[slot] = bit;
                        n += 1;
                    }
                }
            }
            // drain the tail: the last min(n, PREFETCH_DISTANCE) bits are prefetched
            // but not yet set.
            for i in n.saturating_sub(PREFETCH_DISTANCE)..n {
                set.set_at(ring[i & (PREFETCH_DISTANCE - 1)]);
            }
            n
        })
        .sum()
}

/// attempts the bitset path for one shrink-phase round; returns `None` (falling
/// back to the existing sort+dedup+intersect path) below `bitset_threshold`, or
/// wherever the bitset path is disabled entirely (see the module-level comment
/// on the `rayon::prelude::*` import above).
///
/// on success, mutates `visited[remaining - 1]` in place (same effect as the
/// existing `par::intersect_sorted` call it replaces) and returns
/// `(num_moves, intersection)` for logging.
///
/// Note it deliberately does not report a deduped/distinct-key count. Nothing
/// downstream uses one - the intersection below is what the algorithm actually
/// carries forward - and on this path it is not a free by-product of any step the
/// round already performs, unlike the growth phase where the extraction's length
/// gives it away. Obtaining it cost a whole extra summary-guided scan over every
/// touched block purely to fill in one column of a log line.
#[cfg(any(target_arch = "wasm32", target_os = "android"))]
fn try_bitset_shrink_round(
    _keyset: &mut Option<DenseKeySet>,
    _visited: &mut [Vec<Board>],
    _remaining: usize,
    _threads: usize,
    _timer: &mut Timer,
) -> Option<(usize, usize)> {
    None
}

/// keeps the boards of `chunk` that `set` contains, in order - one chunk of the
/// shrink phase's intersect.
///
/// Software-pipelined the same way `generate_into_bitset` pipelines its writes:
/// each board's probe is issued `PREFETCH_DISTANCE` boards before it is needed,
/// so the DRAM round trips overlap instead of serializing. See
/// [`DenseKeySet::prefetch_at`] for why the read side needs this even
/// though, unlike `set`, it is not a barrier.
///
/// Carries each board's bit position in a ring buffer, as the write side does, so
/// that the position is derived once per board rather than once to prefetch it and
/// again to probe it. This used to recompute instead, on the argument that
/// `to_compressed_repr` is a single `pext` and therefore free next to the misses
/// this loop is bound on. That argument was right about the `pext` and missed what
/// came to sit behind it: ranking the key space (see `keyset.rs`) put two
/// bounds-checked table reads between the key and the bit, and profiling showed
/// that duplicated work at ~5.9% of a run rather than ~3%.
///
/// The obvious next step from there is to stop deriving the bit at all - to store
/// `visited` as the `u32` ranks themselves rather than as `Board`s. This loop is
/// where that pays most: the stored value would *be* the bit position, so the
/// `pext`, both table reads and the ring buffer all disappear, and the scan reads 4
/// bytes per board instead of 8. Measured in isolation on the largest round's real
/// data (`examples/probe_width_bench.rs`), that is worth **-46.6%** where nearly
/// everything survives the filter and **-39.2%** at a 76% survival rate, trending
/// down as fewer survive because the narrower output matters less.
///
/// It is still not worth doing, for a reason no amount of tuning this loop changes:
/// the intersect totals **3.76 ms** across the whole run (1.35 ms on its largest
/// round, then 0.95, 0.57, 0.36, and a tail under 0.25), out of ~74 ms. Cutting 40%
/// off that is ~1.5 ms. Against it, a rank only means anything relative to one
/// layer, so every *generation* source - ~5.1M boards over the run - would have to
/// be decoded back through `unrank` + `pdep` before `possible_moves` could touch
/// it, as would the 1.68M-board final flatten, which additionally needs a
/// `high_cum` rebuilt per layer. That is the same order as the win, before counting
/// the standing risk that a rank read against the wrong layer aliases silently onto
/// a different board rather than failing.
///
/// The general lesson is in the 3.76 ms, not in the 46%: this loop was already too
/// small a share of the run to be worth restructuring the data for.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn intersect_chunk(set: &DenseKeySet, chunk: &[Board]) -> Vec<Board> {
    /// how many boards ahead of the probe the prefetch runs. Matches
    /// `generate_into_bitset`'s distance; the two loops are covering the same
    /// latency against comparable amounts of per-item work. Must be a power of two
    /// (the ring index is masked, not divided).
    const PREFETCH_DISTANCE: usize = 16;

    // Pre-sized rather than grown. The filter keeps 12-20% of its input, so this
    // over-allocates by ~5x, but `par::par_join` copies each chunk out and drops
    // it immediately, so the slack is short-lived - and growing instead would put
    // reallocations in the middle of the probe stream this function exists to
    // keep flowing.
    let mut out = Vec::with_capacity(chunk.len());

    // board `j`'s bit position lives in `ring[j % PREFETCH_DISTANCE]` from the
    // moment it is prefetched until it is probed.
    let mut ring = [0u64; PREFETCH_DISTANCE];

    // warm-up: the first `PREFETCH_DISTANCE` probes have nobody ahead of them to
    // have issued their fetch, so issue it here.
    for (slot, board) in chunk.iter().take(PREFETCH_DISTANCE).enumerate() {
        let bit = set.index(board.to_compressed_repr());
        set.prefetch_at(bit);
        ring[slot] = bit;
    }

    // body: probe board `i` while fetching board `i + PREFETCH_DISTANCE`. Both
    // halves are `len - PREFETCH_DISTANCE` long, so for a chunk shorter than the
    // distance this is empty and the drain below handles every board.
    //
    // `get(..).unwrap_or(&[])` rather than `&chunk[PREFETCH_DISTANCE..]`: a range
    // whose *start* is past the end panics rather than yielding an empty slice, so
    // indexing would take down any chunk shorter than the distance even though the
    // loop it feeds would have done nothing. `par::parallel` hands whole (not
    // chunked) inputs to this when they are small, and the shrink phase's later
    // rounds do filter very short ones, so that is a reachable panic and not a
    // theoretical one - it was one, until the test below caught it.
    let split = chunk.len().saturating_sub(PREFETCH_DISTANCE);
    let ahead_of = chunk.get(PREFETCH_DISTANCE..).unwrap_or(&[]);
    for (i, (board, ahead)) in chunk[..split].iter().zip(ahead_of).enumerate() {
        // board `i` and board `i + PREFETCH_DISTANCE` share a ring slot, since the
        // distance is a power of two - so read the one being probed out before the
        // one being prefetched overwrites it.
        let slot = i & (PREFETCH_DISTANCE - 1);
        let bit = ring[slot];
        let ahead_bit = set.index(ahead.to_compressed_repr());
        set.prefetch_at(ahead_bit);
        ring[slot] = ahead_bit;
        if set.test_at(bit) {
            out.push(*board);
        }
    }

    // drain: the last `PREFETCH_DISTANCE` boards were prefetched by the body (or
    // the warm-up), so their bits are already in the ring and only need probing.
    for (i, board) in chunk[split..].iter().enumerate() {
        if set.test_at(ring[(split + i) & (PREFETCH_DISTANCE - 1)]) {
            out.push(*board);
        }
    }

    out
}

/// Marks each board of `states` present in `set`, which must already be on their
/// layer. Pipelined exactly as the move loops are - the keys of an unsorted round
/// scatter over the map, so these are the same DRAM round trips, just one per board
/// instead of one per move.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn fill_bitset(states: &[Board], set: &DenseKeySet) -> usize {
    const PREFETCH_DISTANCE: usize = 16;
    const CHUNK: usize = 2048;

    states.par_chunks(CHUNK).for_each(|chunk| {
        let mut ring = [0u64; PREFETCH_DISTANCE];
        for (slot, board) in chunk.iter().take(PREFETCH_DISTANCE).enumerate() {
            let bit = set.index(board.to_compressed_repr());
            set.prefetch_at(bit);
            ring[slot] = bit;
        }
        let split = chunk.len().saturating_sub(PREFETCH_DISTANCE);
        let ahead_of = chunk.get(PREFETCH_DISTANCE..).unwrap_or(&[]);
        for (i, ahead) in ahead_of.iter().enumerate() {
            let slot = i & (PREFETCH_DISTANCE - 1);
            let bit = ring[slot];
            let ahead_bit = set.index(ahead.to_compressed_repr());
            set.prefetch_at(ahead_bit);
            ring[slot] = ahead_bit;
            set.set_at(bit);
        }
        for i in split..chunk.len() {
            set.set_at(ring[i & (PREFETCH_DISTANCE - 1)]);
        }
    });
    states.len()
}

/// Keeps the boards of `chunk` that have at least one predecessor in `set` - one
/// chunk of the reversed shrink round described on [`try_bitset_shrink_round`].
///
/// `set` holds the *source* layer, so this asks each candidate directly whether any
/// board that could move onto it is present, instead of asking every source board
/// where it can move to. A board's reverse moves are exactly its predecessors: a
/// reverse move at `(idx, dir)` puts back the peg a forward move would have jumped,
/// so `c`'s reverse-move images are precisely the boards with a forward move to `c`.
///
/// Deliberately no early exit on the first hit. It looks free - a kept board could
/// stop after one probe - but only ~11% of candidates are kept here, so the average
/// barely moves, and stopping early means the next probe's address is not known
/// until the current one returns. That is exactly the serialization the prefetch
/// pipeline exists to avoid, so all of a board's predecessors are issued and then
/// tested, and `keep` is ORed into rather than short-circuited.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn reverse_probe_chunk(set: &DenseKeySet, chunk: &[Board]) -> (Vec<Board>, usize) {
    const PREFETCH_DISTANCE: usize = 16;

    let mut keep = vec![false; chunk.len()];
    // a predecessor's bit position, plus which candidate it belongs to - the ring
    // defers each probe past the end of its own board's moves, so the owner has to
    // travel with it
    let mut ring = [(0u64, 0usize); PREFETCH_DISTANCE];
    let mut n = 0usize;
    for (i, board) in chunk.iter().enumerate() {
        let syms = board.symmetries();
        for dir in Dir::enumerate() {
            for idx in board.rev_mov_pattern_mask(dir) {
                let bit = set.index(Board::normalize_after_move(&syms, idx, dir).to_compressed_repr());
                set.prefetch_at(bit);
                let slot = n & (PREFETCH_DISTANCE - 1);
                if n >= PREFETCH_DISTANCE {
                    let (deferred, owner) = ring[slot];
                    keep[owner] |= set.test_at(deferred);
                }
                ring[slot] = (bit, i);
                n += 1;
            }
        }
    }
    for j in n.saturating_sub(PREFETCH_DISTANCE)..n {
        let (deferred, owner) = ring[j & (PREFETCH_DISTANCE - 1)];
        keep[owner] |= set.test_at(deferred);
    }

    let mut out = Vec::with_capacity(keep.iter().filter(|k| **k).count());
    out.extend(
        chunk
            .iter()
            .zip(&keep)
            .filter(|(_, k)| **k)
            .map(|(b, _)| *b),
    );
    (out, n)
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn try_bitset_shrink_round(
    keyset: &mut Option<DenseKeySet>,
    visited: &mut [Vec<Board>],
    remaining: usize,
    threads: usize,
    timer: &mut Timer,
) -> Option<(usize, usize)> {
    if visited[remaining].len() < bitset_threshold() {
        return None;
    }

    // Which way round to run the round.
    //
    // The round has to connect two sets one move apart, and either side can be the
    // one put in the map. The default fills it with every forward move of
    // `visited[remaining]` and probes `visited[remaining - 1]` once per board; the
    // reversed form fills it with `visited[remaining]` itself and probes once per
    // *predecessor* of each `visited[remaining - 1]` board. Both answer the same
    // question and the results are identical.
    //
    // Boards average ~10 moves either way, so the totals are ~10a + b against
    // a + 10b for set sizes a and b: the reversed form is the cheaper one exactly
    // when `a >= b`. That is a single round here - the first, where the inverse step
    // has just made the two sides the same size - and the later rounds, whose source
    // side has been cut to a fraction of the side it probes, keep the default.
    //
    // At equal sizes the operation counts tie, and what breaks the tie is the mix:
    // filling costs one `lock or` per board and probing one load per move, so the
    // reversed form turns ~17.6M read-modify-writes into loads. That matters beyond
    // their latency, which the prefetch pipeline already hides on both paths - an
    // RMW takes the line exclusively and dirties it, so it costs a fetch and an
    // eventual writeback where a load costs only the fetch, and it contends with the
    // other 15 threads for ownership of lines they are also touching.
    if visited[remaining].len() >= visited[remaining - 1].len() {
        return try_bitset_shrink_round_reversed(keyset, visited, remaining, threads, timer);
    }

    // a forward move removes the jumped peg, so the keys this round stores - and the
    // `visited[remaining - 1]` side it probes with them - are one peg lighter than
    // the boards generating them. `begin_round` needs that count to rank by, and
    // clears the map as part of switching to it (see `keyset.rs`).
    let pegs = visited[remaining][0].count_pegs() - 1;
    debug_assert!(
        visited[remaining]
            .iter()
            .all(|b| b.count_pegs() == pegs + 1)
            && visited[remaining - 1]
                .iter()
                .all(|b| b.count_pegs() == pegs),
        "a BFS round must hold one peg count throughout, or ranking aliases boards"
    );
    let set = keyset.get_or_insert_with(DenseKeySet::new);
    set.begin_round(pegs);

    let num_moves = generate_into_bitset(&visited[remaining], set, true);
    // see the matching comment in the pre-existing sort+dedup path below: this
    // index is never read again once this round has read it, except by the final
    // flatten+collect step, which only reads indices 0..=(Board::SLOTS - 1) / 2.
    if remaining == (Board::SLOTS - 1) / 2 + 1 {
        visited[remaining] = Vec::new();
    }
    timer.round("moves".into());

    // probing the (already small) growth-phase side against this round's bitset
    // is cheaper than extracting the bitset into a sorted Vec just to merge-
    // intersect it against `visited[remaining - 1]` the way the non-bitset path
    // does - no extraction needed at all here.
    visited[remaining - 1] = par::parallel(&visited[remaining - 1], threads, |chunk| {
        intersect_chunk(set, chunk)
    });
    let intersection = visited[remaining - 1].len();
    timer.round("intersect".into());

    Some((num_moves, intersection))
}

/// One shrink round run with the sides swapped - see the dispatch comment in
/// [`try_bitset_shrink_round`] for when and why this is the cheaper direction.
///
/// The map holds `visited[remaining]` itself, on *its* layer rather than the layer
/// below, and each `visited[remaining - 1]` board asks whether any of its
/// predecessors is present. No moves are generated into the map at all.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn try_bitset_shrink_round_reversed(
    keyset: &mut Option<DenseKeySet>,
    visited: &mut [Vec<Board>],
    remaining: usize,
    threads: usize,
    timer: &mut Timer,
) -> Option<(usize, usize)> {
    // the map holds the source boards unchanged, so it ranks by *their* peg count -
    // one heavier than the candidates probing it, and the opposite of the default
    // path's choice
    let pegs = visited[remaining][0].count_pegs();
    debug_assert!(
        visited[remaining].iter().all(|b| b.count_pegs() == pegs)
            && visited[remaining - 1]
                .iter()
                .all(|b| b.count_pegs() == pegs - 1),
        "a BFS round must hold one peg count throughout, or ranking aliases boards"
    );
    let set = keyset.get_or_insert_with(DenseKeySet::new);
    set.begin_round(pegs);

    fill_bitset(&visited[remaining], set);
    // same early free as the default path: this index is dead once the round has
    // read it, except for the final flatten, which stops below it
    if remaining == (Board::SLOTS - 1) / 2 + 1 {
        visited[remaining] = Vec::new();
    }
    timer.round("moves".into());

    let probed = std::sync::atomic::AtomicUsize::new(0);
    visited[remaining - 1] = par::parallel(&visited[remaining - 1], threads, |chunk| {
        let (kept, n) = reverse_probe_chunk(set, chunk);
        // one add per chunk, not per probe
        probed.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
        kept
    });
    let intersection = visited[remaining - 1].len();
    timer.round("intersect".into());

    // the moves this round considered are the predecessors it probed, which is the
    // same quantity the default path reports (moves examined, pre-dedup) counted on
    // the other side of the round
    Some((probed.into_inner(), intersection))
}

/// attempts the bitset path for one growth-phase round; see [`try_bitset_shrink_round`].
///
/// unlike the shrink phase, the result here must persist as `visited[i + 1]` for
/// many later rounds, so (unlike the shrink phase's probe-only approach) this
/// does need to extract the bitset into a sorted `Vec<Board>`.
#[cfg(any(target_arch = "wasm32", target_os = "android"))]
fn try_bitset_growth_round(
    _keyset: &mut Option<DenseKeySet>,
    _states: &[Board],
    _timer: &mut Timer,
) -> Option<(usize, Vec<Board>)> {
    None
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn try_bitset_growth_round(
    keyset: &mut Option<DenseKeySet>,
    states: &[Board],
    timer: &mut Timer,
) -> Option<(usize, Vec<Board>)> {
    if states.len() < bitset_threshold() {
        return None;
    }

    // a reverse move puts a peg back, so this round's keys are one peg heavier than
    // the boards generating them; see the shrink round's matching comment.
    let pegs = states[0].count_pegs() + 1;
    debug_assert!(
        states.iter().all(|b| b.count_pegs() == pegs - 1),
        "a BFS round must hold one peg count throughout, or ranking aliases boards"
    );
    let set = keyset.get_or_insert_with(DenseKeySet::new);
    set.begin_round(pegs);

    let num_moves = generate_into_bitset(states, set, false);
    timer.round("reverse".into());

    // pruned during extraction rather than by a second pass over the result - see
    // `DenseKeySet::drain_sorted_by_key`, which explains what that saves
    let deduped = set.drain_sorted_by_key(growth_survives_pagoda);
    timer.round("dedup".into());

    Some((num_moves, deduped))
}

fn possible_moves(states: &[Board]) -> Vec<Board> {
    let mut constellations = Board::possible_moves(states);
    Board::normalize_all(&mut constellations);
    constellations
}

#[cfg(target_arch = "wasm32")]
fn possible_moves_par(states: &[Board], _: usize) -> Vec<Board> {
    possible_moves(states)
}

#[cfg(not(target_arch = "wasm32"))]
fn possible_moves_par(states: &[Board], num_threads: usize) -> Vec<Board> {
    par::parallel(states, num_threads, possible_moves)
}

fn reverse_moves(states: &[Board]) -> Vec<Board> {
    let mut constellations = Board::possible_reverse_moves(states);
    Board::normalize_all(&mut constellations);
    constellations
}

#[cfg(target_arch = "wasm32")]
fn reverse_moves_par(states: &[Board], _: usize) -> Vec<Board> {
    reverse_moves(states)
}

#[cfg(not(target_arch = "wasm32"))]
fn reverse_moves_par(states: &[Board], num_threads: usize) -> Vec<Board> {
    par::parallel(states, num_threads, reverse_moves)
}

fn inverse_normalized(states: &[Board]) -> Vec<Board> {
    let mut inverted: Vec<Board> = states.iter().map(|b| b.inverse()).collect();
    Board::normalize_all(&mut inverted);
    inverted
}

#[cfg(target_arch = "wasm32")]
fn inverse_normalized_par(states: &[Board], _: usize) -> Vec<Board> {
    inverse_normalized(states)
}

#[cfg(not(target_arch = "wasm32"))]
fn inverse_normalized_par(states: &[Board], num_threads: usize) -> Vec<Board> {
    par::parallel(states, num_threads, inverse_normalized)
}

/// concatenates the finished BFS layers into one vector.
///
/// `layers.iter().flatten().collect()` would do it in one line and was what stood
/// here, but it is the worst available shape for this: `Flatten` cannot report a
/// useful lower bound to `size_hint`, so `collect` cannot pre-size and instead grows
/// the destination by repeated reallocation - copying the prefix again each time -
/// all on one thread while the other fifteen idle.
///
/// [`par::par_join`] is the operation this actually is, and already exists for the
/// same reason in `keyset.rs`'s extraction: sum the lengths, allocate once, then
/// have rayon copy the layers into their slots concurrently.
#[cfg(target_arch = "wasm32")]
fn flatten_layers(layers: &[Vec<Board>]) -> Vec<Board> {
    layers.concat()
}

#[cfg(not(target_arch = "wasm32"))]
fn flatten_layers(layers: &[Vec<Board>]) -> Vec<Board> {
    par::par_join(layers)
}

fn expand_with_inverse(states: &[Board]) -> Vec<Board> {
    states
        .iter()
        .flat_map(|b| [*b, b.inverse().normalize()])
        .collect()
}

#[cfg(target_arch = "wasm32")]
fn expand_with_inverse_par(states: &[Board], _: usize) -> Vec<Board> {
    expand_with_inverse(states)
}

#[cfg(not(target_arch = "wasm32"))]
fn expand_with_inverse_par(states: &[Board], num_threads: usize) -> Vec<Board> {
    par::parallel(states, num_threads, expand_with_inverse)
}

pub fn calculate_feasible_set(threads: Option<NonZero<usize>>) -> Vec<Board> {
    let mut timer = Timer::new();
    let threads = threads.unwrap_or(par::num_threads()).get();
    #[cfg(not(target_arch = "wasm32"))]
    par::configure_thread_pool(threads);
    let mut visited = vec![vec![], vec![Board::solved()]];
    let mut sort_time = Duration::ZERO;
    let mut keyset: Option<DenseKeySet> = None;

    let mut total_constellations = 0;
    let mut total_moves = 0;
    info!(
        "{:>10} {:>10} {:>10}         {:>10}",
        "boards", "moves", "deduped", "intersection"
    );
    info!("-----------------------------------------------------");
    for i in 1..(Board::SLOTS - 1) / 2 {
        let mut timer = Timer::new();

        let num_constellations = visited[i].len();

        let (num_moves, constellations) =
            if let Some(result) = try_bitset_growth_round(&mut keyset, &visited[i], &mut timer) {
                result
            } else {
                let mut constellations: Vec<Board> = reverse_moves_par(&visited[i], threads);
                timer.round("reverse".into());

                let num_moves = constellations.len();

                constellations.fast_sort_unstable_mt(threads);
                timer.round("sort".into());

                let constellations = constellations.par_dedup(threads);

                // pagoda-function pruning (see pagoda.rs), applied to the DEDUPED
                // set rather than fused into generation: checking pagoda per raw
                // move instead measurably regressed wall time - the check was then
                // paid for every one of the 55-85% of raw moves that turn out to be
                // duplicates of an already-seen board, instead of once per distinct
                // board. Parallel filter rather than `Vec::retain`, which is
                // single-threaded.
                //
                // The bitset path prunes inside its extraction instead, which avoids
                // both this pass and the buffer it writes into; this path has no
                // equivalent hook. It only runs for rounds under
                // `bitset_threshold` now, so the difference is small either way.
                //
                // Purely an optimization, on both paths: it shrinks the intermediate
                // sets, but the shrink phase overwrites every `visited` index it
                // feeds with an exact intersection, so dropping it entirely still
                // yields 1679072 - verified. What it buys is smaller intermediates,
                // not correctness.
                let constellations =
                    par::par_filter(&constellations, threads, |&b| growth_survives_pagoda(b));
                timer.round("dedup".into());

                (num_moves, constellations)
            };

        let deduped = constellations.len();
        visited.push(constellations);

        total_moves += num_moves;
        total_constellations += deduped;

        info!(
            "{num_constellations:>10} {num_moves:>10} {deduped:>10} ({:>5.1}%)                        {:>12?} (r: {:>12?}, s: {:>12?}, d: {:>12?})",
            deduped as f64 / num_moves as f64 * 100.,
            timer.total(),
            timer.category("reverse".into()),
            timer.category("sort".into()),
            timer.category("dedup".into()),
        );
        sort_time += timer.category("sort".into());
    }

    timer.round("reverse step".into());

    let inverted = inverse_normalized_par(&visited[(Board::SLOTS - 1) / 2], threads);
    // Deliberately NOT sorted, and it used to be. Nothing requires this vector
    // ordered: it is only ever a generation source (the first shrink round below
    // reads it and then frees it), and the final collect takes indices
    // 0..=(SLOTS - 1) / 2, so it is dropped without being read again. That holds
    // whatever `bitset_threshold` is - the sort path sorts the moves it derives
    // from this, and the `visited[16]` it merges against comes from the growth
    // phase already ordered - so no setting of that knob needs this ordered either.
    //
    // The sort was here for the *locality* of the round that consumes it, which is
    // the largest of the run: 2.0M boards generating 19.7M keys. `inverse` and
    // `normalize` are not monotonic in the compressed key, so without it the source
    // is unordered with respect to its own keys and those writes scatter over the
    // map instead of sweeping it. That was worth keeping when the map was 1 GiB,
    // and ranking the key space is what killed it - a 5.4x smaller touched
    // footprint means scattered writes cost much less to begin with. Measured
    // across the two regimes, medians:
    //
    //     map          sort costs   locality buys back   net        reps
    //     1 GiB          3.07 ms          2.62 ms        -0.76 ms   6 of 12
    //     139 MiB        3.36 ms          1.55 ms        -1.81 ms   14 of 14
    //
    // So it went from ~85% self-financing (a wash, kept) to ~46% (a clear loss,
    // removed): -1.81 ms median, -2.26 ms on minima. The penalty it defends against
    // has now shrunk twice - `generate_into_bitset`'s prefetch pipeline first hid
    // most of the miss latency (a comparable round once went 28 -> 54 ms unsorted),
    // then ranking shrank the misses themselves.
    //
    // Worth restoring if the map ever grows again, since that reverses the trade.
    visited.push(inverted);

    timer.round("inverse step".into());

    for remaining in (2..=(Board::SLOTS - 1) / 2 + 1).rev() {
        let mut timer = Timer::new();

        let num_constellations = visited[remaining].len();

        // `deduped` is `None` on the bitset path, which does not count its distinct
        // keys - see `try_bitset_shrink_round`. The sort path gets the count for
        // free as the length of a `Vec` it already built.
        let (num_moves, deduped, intersection) = if let Some((num_moves, intersection)) =
            try_bitset_shrink_round(&mut keyset, &mut visited, remaining, threads, &mut timer)
        {
            (num_moves, None, intersection)
        } else {
            let mut constellations = possible_moves_par(&visited[remaining], threads);
            // Every other `visited[remaining]` gets overwritten with its final,
            // validated value one iteration later (as `visited[remaining - 1]` of
            // the *next* iteration) and is still needed by the final collect step
            // below (which reads indices 0..=(Board::SLOTS - 1) / 2). Only the very
            // first iteration's index — one past that range — is truly dead after
            // this read, so only it is safe to free early.
            if remaining == (Board::SLOTS - 1) / 2 + 1 {
                visited[remaining] = Vec::new();
            }

            timer.round("moves".into());

            let num_moves = constellations.len();

            constellations.fast_sort_unstable_mt(threads);
            let constellations = constellations.par_dedup(threads);

            // No pagoda-function pruning here, unlike the growth phase. It does
            // prune - these are forward moves, so they travel away from the solved
            // position and can land on unsolvable boards, which is exactly what
            // `pagoda(b) >= pagoda(solved)` detects - but only ~1.8% of the set, and
            // every board it removes would be removed anyway by the intersect
            // below: `visited[remaining - 1]` holds only solvable boards, so the
            // intersect already eliminates *all* unsolvable candidates exactly,
            // whereas pagoda catches a subset. Its only possible payoff was a
            // cheaper intersect, and the intersect costs just 0.3-1.2ms per round,
            // so trimming 1.8% off it cannot repay a filter pass over up to ~1M
            // elements (which evaluates pagoda twice per element, once to size the
            // output). Measured: removing it is a net win.
            let deduped = constellations.len();

            timer.round("sort".into());

            visited[remaining - 1] =
                par::intersect_sorted(&visited[remaining - 1], &constellations, threads);
            let intersection = visited[remaining - 1].len();

            timer.round("intersect".into());

            (num_moves, Some(deduped), intersection)
        };

        total_moves += num_moves;

        // both columns that depend on the deduped count collapse to "-" when it
        // wasn't computed; widths match the populated case to keep the table aligned
        let (deduped_col, intersection_pct) = match deduped {
            Some(deduped) => (
                format!(
                    "{deduped:>10} ({:>5.1}%)",
                    deduped as f64 / num_moves as f64 * 100.
                ),
                format!("({:>5.1}%)", intersection as f64 / deduped as f64 * 100.),
            ),
            None => (format!("{:>10} {:>8}", "-", ""), format!("{:>8}", "")),
        };

        info!(
            "{num_constellations:>10} {num_moves:>10} {deduped_col} {intersection:>10} {intersection_pct}    {:>12?} (m: {:>12?}, s: {:>12?}, i: {:>12?})",
            timer.total(),
            timer.category("moves".into()),
            timer.category("sort".into()),
            timer.category("intersect".into()),
        );
        sort_time += timer.category("sort".into());
    }

    timer.round("forward".into());

    let take_n = (Board::SLOTS - 1) / 2 + 1;
    let flattened = flatten_layers(&visited[..take_n]);
    // freed before `expand_with_inverse_par` allocates its (2x larger) output rather
    // than at the end of the function: borrowing the layers to flatten them, instead
    // of consuming them as `into_iter().flatten()` did, otherwise keeps all ~41 MB
    // of them alive across that allocation for no reason.
    drop(visited);
    timer.round("flatten".into());

    let solvable = expand_with_inverse_par(&flattened, threads);

    timer.round("collect".into());

    assert_eq!(solvable.len(), 1679072);
    info!("analyzed {total_moves} moves and {total_constellations} different constellations");
    for (desc, dur) in timer.descriptions().zip(timer.durations()) {
        info!("{desc:>15}: {dur:>12?}");
    }
    info!("          total: {:>12?}", timer.total());
    info!("        sorting: {sort_time:?}");
    solvable
}

#[cfg(all(test, not(any(target_arch = "wasm32", target_os = "android"))))]
mod tests {
    use super::*;

    /// The straight-line generator `generate_into_bitset` replaced: same keys, no
    /// ring buffer, no prefetch, no chunking.
    fn generate_reference(states: &[Board], set: &DenseKeySet, forward: bool) -> usize {
        let mut count = 0usize;
        for board in states {
            for dir in Dir::enumerate() {
                let mask = if forward {
                    board.mov_pattern_mask(dir)
                } else {
                    board.rev_mov_pattern_mask(dir)
                };
                for idx in mask {
                    let moved = board.toggle_mov_idx_unchecked(idx, dir).normalize();
                    set.set(moved.to_compressed_repr());
                    count += 1;
                }
            }
        }
        count
    }

    /// `levels` rounds of reverse moves out from the solved board, deduped - i.e.
    /// exactly the kind of input the bitset rounds see.
    fn states_after(levels: usize) -> Vec<Board> {
        let mut states = vec![Board::solved()];
        for _ in 0..levels {
            let mut next = reverse_moves(&states);
            next.fast_sort_unstable_mt(1);
            next.dedup();
            states = next;
        }
        states
    }

    fn assert_matches_reference(states: &[Board], forward: bool) {
        let mut pipelined = DenseKeySet::new();
        let mut reference = DenseKeySet::new();
        // the map is ranked within one peg count, so it has to be told which; a
        // forward move takes a peg off the generating boards, a reverse one adds it
        let pegs = if forward {
            states[0].count_pegs() - 1
        } else {
            states[0].count_pegs() + 1
        };
        pipelined.begin_round(pegs);
        reference.begin_round(pegs);
        let n = generate_into_bitset(states, &pipelined, forward);
        let n_ref = generate_reference(states, &reference, forward);
        assert_eq!(n, n_ref, "move count differs ({} boards)", states.len());
        assert_eq!(
            pipelined.drain_sorted_by_key(|_| true),
            reference.drain_sorted_by_key(|_| true),
            "key set differs ({} boards, forward={forward})",
            states.len()
        );
    }

    /// The reversed round is taken by exactly one round of a real run, so a defect
    /// in it would be a single wrong intersection buried in the middle of the
    /// pipeline - the kind that surfaces as a wrong final count with nothing to
    /// point at. Both directions answer the same question, so they can simply be
    /// run against identical inputs and compared.
    #[test]
    fn reversed_shrink_round_matches_the_default() {
        // `src` is deliberately only part of its round, so that a good fraction of
        // the candidates have no predecessor in it and the filter actually filters -
        // comparing two implementations that both keep everything proves nothing. A
        // quarter also keeps it under the round below, which is what sends
        // `try_bitset_shrink_round` down the default path rather than dispatching
        // straight back to the one being compared against.
        let full = states_after(10);
        let src = full[..full.len() / 4].to_vec();
        let candidates = states_after(9);
        let pegs = src[0].count_pegs();
        assert_eq!(candidates[0].count_pegs(), pegs - 1);
        assert!(src.len() >= bitset_threshold(), "round too small for either path");
        assert!(
            src.len() < candidates.len(),
            "sizes must send `try_bitset_shrink_round` down its default path"
        );

        let mut default = vec![Vec::new(); pegs + 1];
        default[pegs] = src.clone();
        default[pegs - 1] = candidates.clone();
        let mut reversed = default.clone();

        // one map for both: `begin_round` clears it and switches layers, which is
        // exactly the transition being relied on here
        let mut keyset = None;
        let a = try_bitset_shrink_round(&mut keyset, &mut default, pegs, 1, &mut Timer::new());
        let b =
            try_bitset_shrink_round_reversed(&mut keyset, &mut reversed, pegs, 1, &mut Timer::new());

        let (a, b) = (a.expect("default path declined"), b.expect("reversed path declined"));
        assert_eq!(
            default[pegs - 1],
            reversed[pegs - 1],
            "reversed round kept a different set ({} vs {})",
            default[pegs - 1].len(),
            reversed[pegs - 1].len()
        );
        assert_eq!(a.1, b.1, "reported intersection sizes differ");
        assert!(
            default[pegs - 1].len() < candidates.len(),
            "filter kept everything, so this would pass even if it did nothing"
        );
    }

    /// The ring buffer defers every `set()` by `PREFETCH_DISTANCE`, so a key is
    /// only written because a later key displaced it or because the drain caught
    /// it. Both an off-by-one in the drain bound and any counter shared between
    /// "moves seen" and "ring slots written" would silently drop keys here while
    /// leaving the code looking right - and, since the bitset is a superset filter
    /// for the rounds that use it, a dropped key shows up as a wrong final answer
    /// rather than a crash.
    #[test]
    fn pipelined_generate_matches_straight_line() {
        // ~9.7k boards: several `CHUNK`s, each holding many times PREFETCH_DISTANCE
        // keys, so the steady-state path dominates and chunk boundaries are crossed.
        let states = states_after(8);
        assert!(
            states.len() > 2048,
            "want multiple chunks, got {}",
            states.len()
        );
        assert_matches_reference(&states, false);
        assert_matches_reference(&states, true);
    }

    /// A chunk that never fills the ring exercises only the drain, which the test
    /// above cannot reach.
    #[test]
    fn pipelined_generate_matches_straight_line_below_prefetch_distance() {
        for levels in 0..4 {
            let states = states_after(levels);
            assert_matches_reference(&states, false);
            assert_matches_reference(&states, true);
        }
    }

    /// `intersect_chunk` splits its input into warm-up, a `zip`-driven body and a
    /// drain, so - exactly as with the generator above - a boundary that is off by
    /// one silently drops or duplicates boards while the code still looks right,
    /// and the result is a wrong final answer rather than a crash. Checks every
    /// length across both boundaries, plus lengths well past them.
    #[test]
    fn intersect_chunk_matches_straight_line_filter() {
        let states = states_after(6);
        let mut set = DenseKeySet::new();
        // these keys are the boards themselves, so the layer is their own peg count
        set.begin_round(states[0].count_pegs());
        // put half the boards in the set, so the filter has to both keep and drop,
        // and interleaved so it cannot pass by accident on a contiguous run
        for board in states.iter().step_by(2) {
            set.set(board.to_compressed_repr());
        }
        for len in (0..40).chain([100, 512, states.len()]) {
            let chunk = &states[..len.min(states.len())];
            let expected: Vec<Board> = chunk
                .iter()
                .copied()
                .filter(|b| set.test(b.to_compressed_repr()))
                .collect();
            assert_eq!(
                intersect_chunk(&set, chunk),
                expected,
                "intersect_chunk differs at len {}",
                chunk.len()
            );
        }
    }
}
