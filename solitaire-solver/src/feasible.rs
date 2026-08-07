use std::time::Duration;

use std::num::NonZero;

use log::info;

// the dense-bitset path (see keyset.rs) is off on wasm32 (a flat 1 GiB
// allocation is a non-starter in a browser) and, for now, on Android too:
// solitaire-game calls calculate_feasible_set on startup on a real,
// potentially memory-constrained device that hasn't been tested against this
// allocation - revisit once that's been measured on a representative device.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
use rayon::prelude::*;

use crate::{
    Board, Dir,
    keyset::DenseKeySet,
    par::{self, ParDedup},
    sort::Sort,
    timer::Timer,
};

/// boards-in-round threshold above which a shrink-phase round uses the dense
/// bitset (see `keyset.rs`) instead of sort+dedup+merge-intersect. Tuned from the
/// real per-round trace (`RUST_LOG=info`): starts by capturing only the single
/// biggest round, which alone accounts for most of the sort+dedup time.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
const BITSET_THRESHOLD: usize = 1_000_000;

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
/// Returns the total number of moves considered (pre-dedup).
///
/// Generation and the bitset writes are software-pipelined `PREFETCH_DISTANCE`
/// keys apart, via the ring buffer below: each key is prefetched (see
/// [`DenseKeySet::prefetch_for_set`]) as soon as it is computed, and only
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
                for dir in Dir::enumerate() {
                    let mask = if forward {
                        board.mov_pattern_mask(dir)
                    } else {
                        board.rev_mov_pattern_mask(dir)
                    };
                    for idx in mask {
                        let moved = board.toggle_mov_idx_unchecked(idx, dir).normalize();
                        let key = moved.to_compressed_repr();
                        set.prefetch_for_set(key);
                        let slot = n & (PREFETCH_DISTANCE - 1);
                        // once the ring is full, the slot about to be overwritten
                        // holds the key prefetched exactly PREFETCH_DISTANCE keys ago
                        if n >= PREFETCH_DISTANCE {
                            set.set(ring[slot]);
                        }
                        ring[slot] = key;
                        n += 1;
                    }
                }
            }
            // drain the tail: the last min(n, PREFETCH_DISTANCE) keys are prefetched
            // but not yet set.
            for i in n.saturating_sub(PREFETCH_DISTANCE)..n {
                set.set(ring[i & (PREFETCH_DISTANCE - 1)]);
            }
            n
        })
        .sum()
}

/// attempts the bitset path for one shrink-phase round; returns `None` (falling
/// back to the existing sort+dedup+intersect path) below `BITSET_THRESHOLD`, or
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

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn try_bitset_shrink_round(
    keyset: &mut Option<DenseKeySet>,
    visited: &mut [Vec<Board>],
    remaining: usize,
    threads: usize,
    timer: &mut Timer,
) -> Option<(usize, usize)> {
    if visited[remaining].len() < BITSET_THRESHOLD {
        return None;
    }

    let already_initialized = keyset.is_some();
    let set = keyset.get_or_insert_with(DenseKeySet::new);
    if already_initialized {
        set.clear();
    }

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
        chunk
            .iter()
            .copied()
            .filter(|b| set.test(b.to_compressed_repr()))
            .collect()
    });
    let intersection = visited[remaining - 1].len();
    timer.round("intersect".into());

    Some((num_moves, intersection))
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
    if states.len() < BITSET_THRESHOLD {
        return None;
    }

    let already_initialized = keyset.is_some();
    let set = keyset.get_or_insert_with(DenseKeySet::new);
    if already_initialized {
        set.clear();
    }

    let num_moves = generate_into_bitset(states, set, false);
    timer.round("reverse".into());

    let deduped = set.extract_sorted_by_key();
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
                timer.round("dedup".into());

                (num_moves, constellations)
            };

        // pagoda-function pruning (see pagoda.rs), applied to the DEDUPED set
        // rather than fused into generation: checking pagoda per-raw-move instead
        // (before dedup) measurably regressed wall time - the check was then paid
        // for every one of the 55-85% of raw moves that turn out to be duplicates
        // of an already-seen board, instead of once per distinct board. Parallel
        // filter instead of Vec::retain (single-threaded) - up to ~2.6M elements
        // on the biggest rounds is enough for that gap to matter on its own.
        let solved_weight = crate::pagoda::pagoda(Board::solved());
        let constellations = par::par_filter(&constellations, threads, |&b| {
            crate::pagoda::pagoda(b.inverse()) >= solved_weight
        });

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

    let mut inverted = inverse_normalized_par(&visited[(Board::SLOTS - 1) / 2], threads);
    inverted.fast_sort_unstable_mt(threads);
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
    let flattened: Vec<Board> = visited.into_iter().take(take_n).flatten().collect();
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
        let n = generate_into_bitset(states, &pipelined, forward);
        let n_ref = generate_reference(states, &reference, forward);
        assert_eq!(n, n_ref, "move count differs ({} boards)", states.len());
        assert_eq!(
            pipelined.extract_sorted_by_key(),
            reference.extract_sorted_by_key(),
            "key set differs ({} boards, forward={forward})",
            states.len()
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
}
