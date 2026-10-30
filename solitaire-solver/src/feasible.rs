//! The feasible set: every board that lies on some complete solution, i.e. that is
//! reachable from the start position *and* can reach the solved position.
//!
//! The two halves of that definition are duals of each other. Reversing a move
//! sequence and complementing every board along it gives a legal move sequence
//! the other way, and the start board is the complement of the solved one. So a
//! board `b` is reachable from the start exactly when `b.inverse()` can reach the
//! solved position - and the two conditions can be checked with *one* search:
//!
//! 1. **Growth.** Breadth-first search backwards from the solved board, one layer
//!    per peg count, up to the middle layer of 16 pegs. Layer `k` is every board of
//!    `k` pegs that can reach the solved position - pruned by the pagoda function
//!    (see `pagoda.rs`), which is an exact test that discards boards whose inverse
//!    cannot reach it, i.e. that are not reachable from the start.
//! 2. **Bridge.** A 16-peg board `b` is reachable from the start iff `b.inverse()`
//!    (17 pegs) can reach the solved position, iff some forward successor of
//!    `b.inverse()` - a 16-peg board - is in layer 16. So the feasible 16-peg
//!    boards are found by probing layer 16 against itself.
//! 3. **Shrink.** Down from there, the feasible boards of layer `k` are exactly the
//!    forward successors of the feasible boards of layer `k + 1` that lie in layer
//!    `k` (they are then reachable from the start, and can reach the solved
//!    position by being in the layer).
//! 4. The feasible boards of 17 to 32 pegs are the inverses of those of 16 to 1.
//!
//! Every layer is held as a bitset keyed by rank (see `keyset.rs`), and is written
//! exactly once, in step 1. Nothing is ever extracted from it into a list: the
//! next growth round scans the set bits and generates from them directly, and the
//! shrink phase probes it one candidate at a time, *taking* each hit so that every
//! feasible board is emitted exactly once. There is no sort, no dedup and no
//! intersection anywhere in the pipeline - the bitsets are the dedup, and the
//! probe is the intersection.
//!
//! This replaced a pipeline that extracted each layer into a sorted `Vec`, then
//! re-filled a single reused bitset per shrink round and intersected the two; see
//! [`calculate_feasible_set`] for what it measured.

use std::num::NonZero;

use log::info;
use rayon::prelude::*;

use crate::{
    Board, Dir,
    keyset::{KeySetPool, LayerView, Tally},
    par,
    timer::Timer,
};

/// The middle layer: the growth phase runs up to it and the bridge round runs on
/// it. 16 for the 33-cell English board.
const MID: usize = (Board::SLOTS - 1) / 2;

/// how many items ahead of the bitset access the prefetch runs, in every pipelined
/// loop below. Must be a power of two (the ring index is masked, not divided).
///
/// Shared by the write side (where a `lock or` is a full barrier and nothing else
/// would overlap the misses) and the read side (where an unpredictable branch per
/// probe keeps the out-of-order window from doing it); see
/// [`LayerView::prefetch_at`]. Both are covering the same latency against
/// comparable per-item work, and 16 is about as many misses as a core keeps in
/// flight anyway.
const PREFETCH_DISTANCE: usize = 16;

/// boards per parallel work unit in the shrink phase, where the sources are a
/// list. Small enough to be a fraction of one thread's share, keeping work-stealing
/// effective, and large enough that the per-chunk `Vec` and the ring drain are
/// amortized.
const CHUNK: usize = 2048;

/// A fixed-size ring that hands each item back exactly `N` pushes after it went in:
/// the software pipeline every bitset loop below is built on.
///
/// A loop computes an item's bit position, issues the prefetch for it, and pushes
/// it; whatever `push` returns was prefetched `N` items ago and is now resident,
/// so it is acted on then. `drain` yields the items still in flight at the end.
struct Ring<T: Copy, const N: usize> {
    slots: [T; N],
    /// also the ring index: every increment corresponds to a slot written, which
    /// is what makes `drain` correct
    n: usize,
}

impl<T: Copy, const N: usize> Ring<T, N> {
    fn new(fill: T) -> Self {
        const { assert!(N.is_power_of_two()) };
        Self {
            slots: [fill; N],
            n: 0,
        }
    }

    /// Stores `item` and returns the item pushed `N` pushes earlier, if any.
    #[inline]
    fn push(&mut self, item: T) -> Option<T> {
        let slot = self.n & (N - 1);
        // once the ring is full, the slot about to be overwritten holds the item
        // pushed exactly N pushes ago
        let out = (self.n >= N).then(|| self.slots[slot]);
        self.slots[slot] = item;
        self.n += 1;
        out
    }

    /// The last `min(pushed, N)` items, oldest first: pushed but not yet returned.
    fn drain(&self) -> impl Iterator<Item = T> + '_ {
        (self.n.saturating_sub(N)..self.n).map(move |i| self.slots[i & (N - 1)])
    }

    /// Total items pushed.
    fn pushed(&self) -> usize {
        self.n
    }
}

/// pagoda-function pruning for the growth phase (see `pagoda.rs`): a board whose
/// inverse cannot reach the solved position is unreachable from it too, so it can
/// never contribute.
///
/// Stated without materializing the inverse: every valid cell is occupied in the
/// full board and invalid cells weigh nothing, so `pagoda(b.inverse())` is just
/// `FULL_WEIGHT - pagoda(b)` (asserted by a test in `pagoda.rs`).
///
/// Applied to the *sources* of a round as they are scanned, never to the moves
/// generated: a tabulated per-move form of this test (one L1 read and an add,
/// evaluated before normalization) prunes ~10% of the move stream and still
/// measured a loss, because the `lock or`s it saves are latency-hidden by the
/// prefetch ring while the test sits on the critical path of every move. The
/// pruned bits are harmless in the layer: a board the pagoda rejects cannot be
/// reached from the start, so no probe from a feasible board can ever land on it.
fn growth_survives_pagoda(board: Board) -> bool {
    crate::pagoda::FULL_WEIGHT - crate::pagoda::pagoda(board) >= crate::pagoda::SOLVED_WEIGHT
}

/// One growth round: every reverse move of every (pagoda-surviving) board of `src`
/// is normalized and set in `dst`, which is the next layer up.
///
/// Generation and the bitset writes are software-pipelined `PREFETCH_DISTANCE`
/// keys apart via a [`Ring`]: each key is prefetched as soon as it is computed,
/// and only `set_at` once that many further keys have been generated. Profiling
/// the straight-line version put ~70% of this function's cycles on the single
/// `lock or` - a serialized round trip per call, because the `lock` prefix bars
/// the overlapping misses that would otherwise hide it. The ~8 symmetries
/// `normalize_after_move` evaluates per key are exactly the independent work
/// needed to cover that latency.
///
/// What the `lock or` still costs once prefetched was measured by replacing it
/// with a plain (racy, wrong) read-modify-write: the growth phase got 20% faster
/// at 6 threads and 22% at one. That bounds what any lock-free scheme can win,
/// and a partitioned scatter - bucket the ranks per thread, then let each thread
/// OR its own region without atomics - would spend most of it on the bucketing
/// and the second pass, so it was not built. The loop is compute-bound at ~27
/// cycles per move; the lock adds ~7 at one thread and ~13 under contention.
///
/// Deliberately no filter of any kind in front of `set_at`. Two were measured and
/// both lost - a recently-seen tag table (the duplicates' reuse distance is far
/// longer than any table that stays in cache: it removed under a fifth of the
/// writes and cost a poorly-predicted branch on all of them) and the per-move
/// pagoda delta described on [`growth_survives_pagoda`]. Nor can the duplicates be
/// avoided at the source: ~6x of the stream is genuine graph convergence and
/// symmetry collapse, neither decidable from `(board, move)` alone.
///
/// Returns `(sources after pruning, moves generated)`.
fn grow(src: LayerView<'_>, dst: LayerView<'_>) -> (usize, usize) {
    let sources = Tally::new();
    let moves = Tally::new();
    src.par_scan(|keys| {
        let mut ring = Ring::<u64, PREFETCH_DISTANCE>::new(0);
        let mut kept = 0usize;
        let mut keys = keys;
        let mut ahead = keys.next();
        while let Some((_, key)) = ahead {
            // decode the next board before generating this one's moves - see
            // `LayerView::par_scan` for why the scan is consumed one board ahead
            ahead = keys.next();
            let board = Board::from_compressed_repr(key);
            if !growth_survives_pagoda(board) {
                continue;
            }
            kept += 1;
            // hoisted out of the move loop: a move is an XOR with a constant mask
            // and the symmetry transforms are GF(2)-linear, so every successor's
            // eight symmetries are these eight XORed against a per-move constant.
            // See `Board::normalize_after_move`.
            let syms = board.symmetries();
            for dir in Dir::enumerate() {
                for idx in board.rev_mov_pattern_mask(dir) {
                    let moved = Board::normalize_after_move(&syms, idx, dir);
                    let bit = dst.index(moved.to_compressed_repr());
                    dst.prefetch_for_write_at(bit);
                    if let Some(ready) = ring.push(bit) {
                        dst.set_at(ready);
                    }
                }
            }
        }
        for bit in ring.drain() {
            dst.set_at(bit);
        }
        sources.add(kept);
        moves.add(ring.pushed());
    });
    (sources.get(), moves.get())
}

/// The bridge round: finds the feasible boards of the middle layer by probing it
/// against itself, marking them in `mark` (the scratch layer, ranked like `mid`).
///
/// A board `b` of the layer can reach the solved position by being in it, and is
/// reachable from the start iff `b.inverse()` can reach the solved position, iff
/// some forward successor `c` of `b.inverse()` is in the layer. Those successors
/// are generated without materializing the inverse: a forward move of
/// `b.inverse()` sits exactly where a reverse move of `b` does, and
/// `g(b.inverse()) == g(b).inverse()` for every symmetry `g`, so the eight
/// symmetries of `b.inverse()` are `b`'s eight XORed with the full board.
///
/// The relation is symmetric: `c` being a successor of `b.inverse()` means
/// `c.inverse()` is a predecessor of `b`, so `c` is reachable from the start too
/// (via `b.inverse()`, which is) - *both* boards of a hit are feasible. And every
/// feasible board is in at least one hit, from either side. So each unordered pair
/// only has to be probed from one side, and a hit marks both boards. Which side is
/// a free choice, and it is not a symmetric one: normalization leaves `c > b` for
/// three quarters of the candidates, so probing only when `c <= b` skips 75% of
/// the ~20M probes this round would otherwise make, for ~1.3M extra marks.
///
/// The skip is a plain branch, and an unpredictable one (75/25, data-dependent).
/// Making it branchless - routing skipped candidates to a bit that is never set
/// and letting them flow through the prefetch and test like the rest - was
/// measured and lost: 130 -> 160 ms for this round at one thread, 30 -> 36 ms at
/// six. The index computation and the ring traffic for the 75% cost more than
/// the mispredicts do.
///
/// Those marks are the round's only writes, and they go to a cold region, so they
/// are pipelined through a second ring with write-intent prefetches rather than
/// done on the spot: a `lock or` to a line that is not resident is a serialized
/// round trip, and 1.3M of them cost more than the probes they follow. A board's
/// own mark is only pushed once however many of its probes hit - its probes come
/// back in order, so "same owner as the last hit" is all that takes to notice.
///
/// Deliberately no early exit once `b` is marked: only ~11% of probes hit, so the
/// average barely moves, and stopping early means the next probe's address is not
/// known until the current one returns - exactly the serialization the prefetch
/// ring exists to avoid.
///
/// Returns the number of probes made.
fn bridge(mid: LayerView<'_>, mark: LayerView<'_>) -> usize {
    /// the pending marks of one task, plus the last board marked on its own behalf
    struct Marks {
        ring: Ring<u64, PREFETCH_DISTANCE>,
        last_owner: u64,
    }
    impl Marks {
        #[inline]
        fn push(&mut self, mark: LayerView<'_>, bit: u64) {
            mark.prefetch_for_write_at(bit);
            if let Some(ready) = self.ring.push(bit) {
                mark.set_at(ready);
            }
        }
    }

    let probes = Tally::new();
    mid.par_scan(|keys| {
        // a probe's bit position, plus the bit of the board it belongs to: the ring
        // defers each probe past the end of its own board's moves, so the owner has
        // to travel with it
        let mut probe_ring = Ring::<(u64, u64), PREFETCH_DISTANCE>::new((0, 0));
        let mut marks = Marks {
            ring: Ring::new(0),
            last_owner: u64::MAX,
        };
        let hit = |probe: u64, owner: u64, marks: &mut Marks| {
            if mid.test_at(probe) {
                marks.push(mark, probe);
                if owner != marks.last_owner {
                    marks.push(mark, owner);
                    marks.last_owner = owner;
                }
            }
        };
        let mut keys = keys;
        let mut ahead = keys.next();
        while let Some((own, key)) = ahead {
            ahead = keys.next();
            let board = Board::from_compressed_repr(key);
            // a rejected board is not reachable from the start, so no hit can involve
            // it; skipping it as a source only saves its probes
            if !growth_survives_pagoda(board) {
                continue;
            }
            let inv_syms = board.symmetries().map(|g| g ^ Board::full());
            for dir in Dir::enumerate() {
                for idx in board.rev_mov_pattern_mask(dir) {
                    let c = Board::normalize_after_move(&inv_syms, idx, dir);
                    if c > board {
                        continue;
                    }
                    let bit = mid.index(c.to_compressed_repr());
                    mid.prefetch_at(bit);
                    if let Some((probe, owner)) = probe_ring.push((bit, own)) {
                        hit(probe, owner, &mut marks);
                    }
                }
            }
        }
        for (probe, owner) in probe_ring.drain() {
            hit(probe, owner, &mut marks);
        }
        for bit in marks.ring.drain() {
            mark.set_at(bit);
        }
        probes.add(probe_ring.pushed());
    });
    probes.get()
}

/// One shrink round: the feasible boards of `layer` are the forward successors of
/// the feasible boards `src` of the layer above that `layer` contains. Each is
/// emitted exactly once, because a hit *takes* the bit (see
/// [`LayerView::take_at`]) and later duplicates then miss.
///
/// Pipelined like the generator: the probe for a candidate is issued
/// `PREFETCH_DISTANCE` candidates before it is tested. The ring carries the
/// candidate board alongside its bit so that a hit can be emitted without
/// unranking the bit.
///
/// The probe is a plain load and the take an RMW. ~10% of probes hit, so testing
/// first keeps the RMW off the 90% that miss - and the hits go through a second
/// ring with a write-intent prefetch, because the line a probe just read is
/// Shared, and a `lock and` on a Shared line is a serialized ownership transfer.
/// Taking on the spot cost about as much as all the probing.
///
/// Returns the feasible boards found, in no particular order, and the probe count.
fn shrink(src: &[Board], layer: LayerView<'_>) -> (Vec<Board>, usize) {
    type Pending = Ring<(u64, Board), PREFETCH_DISTANCE>;
    let probes = Tally::new();
    let chunks: Vec<Vec<Board>> = src
        .par_chunks(CHUNK)
        .map(|chunk| {
            // ~10% of probes hit and boards average ~10 moves, so about one hit per
            // source board; sized for that rather than grown
            let mut out = Vec::with_capacity(chunk.len());
            let mut probe_ring = Pending::new((0, Board::empty()));
            let mut take_ring = Pending::new((0, Board::empty()));
            let hit =
                |probe: u64, candidate: Board, take_ring: &mut Pending, out: &mut Vec<Board>| {
                    if layer.test_at(probe) {
                        layer.prefetch_for_write_at(probe);
                        if let Some((bit, board)) = take_ring.push((probe, candidate))
                            && layer.take_at(bit)
                        {
                            out.push(board);
                        }
                    }
                };
            for board in chunk {
                let syms = board.symmetries();
                for dir in Dir::enumerate() {
                    for idx in board.mov_pattern_mask(dir) {
                        let moved = Board::normalize_after_move(&syms, idx, dir);
                        let bit = layer.index(moved.to_compressed_repr());
                        layer.prefetch_at(bit);
                        if let Some((probe, candidate)) = probe_ring.push((bit, moved)) {
                            hit(probe, candidate, &mut take_ring, &mut out);
                        }
                    }
                }
            }
            for (probe, candidate) in probe_ring.drain() {
                hit(probe, candidate, &mut take_ring, &mut out);
            }
            for (bit, board) in take_ring.drain() {
                if layer.take_at(bit) {
                    out.push(board);
                }
            }
            probes.add(probe_ring.pushed());
            out
        })
        .collect();
    (par::par_join_owned(chunks), probes.get())
}

/// Every board of every layer followed by its normalized inverse, in one
/// allocation: the feasible boards of the upper half of the game are exactly the
/// inverses of the lower half's.
///
/// Written straight into the output - each layer gets its own slice of it, and
/// each chunk of a layer its own slice of that - rather than flattening the layers
/// first and expanding the result: that intermediate is a 6.7 MB allocation that
/// exists only to be read once.
fn collect_with_inverse(layers: &[Vec<Board>]) -> Vec<Board> {
    let lens: Vec<usize> = layers.iter().map(|l| 2 * l.len()).collect();
    let total = lens.iter().sum();
    let mut out = Vec::with_capacity(total);
    let dsts = par::into_mut_slices(out.spare_capacity_mut(), &lens);
    dsts.into_par_iter()
        .zip(layers.par_iter())
        .for_each(|(dst, src)| {
            // SAFETY: `Board: Copy` has no drop glue, so assigning over the
            // uninitialized slots runs no destructor, and every slot is written
            // below before `set_len` exposes it: `dst` is exactly `2 * src.len()`
            // long and the chunk pairing covers it.
            let dst: &mut [Board] = unsafe { std::mem::transmute(dst) };
            dst.par_chunks_mut(2 * CHUNK)
                .zip(src.par_chunks(CHUNK))
                .for_each(|(d, s)| {
                    for (pair, b) in d.as_chunks_mut::<2>().0.iter_mut().zip(s) {
                        *pair = [*b, b.inverse().normalize()];
                    }
                });
        });
    // SAFETY: every layer covered a disjoint span of `0..total` and filled it.
    unsafe { out.set_len(total) };
    out
}

/// Computes the feasible set; see the module documentation for the algorithm.
///
/// Measured against the pipeline it replaced (`--repeat 3`, third iteration's
/// internal timer, interleaved runs, medians; the old phases mapped onto these -
/// its "reverse step" included extracting each layer, its first forward round is
/// the bridge):
///
/// ```text
///   6 P-cores pinned          before      after
///   setup                        -       0.5 ms
///   growth (to layer 16)      68.3 ms   47.3 ms
///   bridge (round 17)         26   ms   25.1 ms
///   shrink (rounds <17)       15   ms   12.4 ms
///   output                     2.9 ms    1.3 ms
///   total                    114.4 ms   88.8 ms    -22%, faster in 8 of 8
///
///   all 16 CPUs (6P + 8E + 2 LP-E)
///   total                     85.1 ms   62.0 ms    -27%, faster in 8 of 8
///
///   one thread, pinned to a P-core (the wasm build's situation)
///   total                    ~405 ms   ~395 ms    parity, growth -15 ms
/// ```
///
/// Where it comes from: growth no longer extracts a layer into a sorted list to
/// generate from it, the shrink rounds no longer fill a bitset and intersect it
/// (one probe per candidate instead), and the first shrink round neither inverts
/// the layer nor fills anything, and probes a quarter as often. The per-move
/// compute is the same everywhere - ~27 cycles of normalization and ranking - and
/// is now the floor; see `grow` for the measurement that puts the `lock or` at a
/// fifth of growth and rules out a lock-free scatter.
pub fn calculate_feasible_set(threads: Option<NonZero<usize>>) -> Vec<Board> {
    let mut timer = Timer::new();
    let threads = threads.unwrap_or(par::num_threads()).get();
    #[cfg(not(target_arch = "wasm32"))]
    par::configure_thread_pool(threads);

    let mut pool = KeySetPool::new(MID);
    timer.round("setup".into());

    // layer 1 is the solved board alone
    {
        let layer = pool.layer(1);
        layer.set_at(layer.index(Board::solved().to_compressed_repr()));
    }

    let mut total_moves = 0;
    info!(
        "{:>5} {:>10} {:>10} {:>12}",
        "layer", "sources", "moves", "time"
    );
    for pegs in 1..MID {
        let round = Timer::new();
        let (sources, moves) = grow(pool.layer(pegs), pool.layer(pegs + 1));
        total_moves += moves;
        info!(
            "{pegs:>5} {sources:>10} {moves:>10} {:>12?}",
            round.elapsed()
        );
    }
    timer.round("growth".into());

    let mut feasible: Vec<Vec<Board>> = vec![Vec::new(); MID + 1];
    let probes = bridge(pool.layer(MID), pool.scratch());
    total_moves += probes;
    feasible[MID] = pool.drain_scratch();
    info!(
        "{MID:>5} {:>10} {probes:>10} {:>12?}   {} feasible",
        "-",
        timer.elapsed_since_round(),
        feasible[MID].len()
    );
    timer.round("bridge".into());

    for pegs in (1..MID).rev() {
        let round = Timer::new();
        let (found, probes) = shrink(&feasible[pegs + 1], pool.layer(pegs));
        total_moves += probes;
        info!(
            "{pegs:>5} {:>10} {probes:>10} {:>12?}   {} feasible",
            feasible[pegs + 1].len(),
            round.elapsed(),
            found.len()
        );
        feasible[pegs] = found;
    }
    timer.round("shrink".into());

    // the bitsets are dead now; free them before the output is allocated rather
    // than at the end of the function, so the two never coexist
    drop(pool);
    let solvable = collect_with_inverse(&feasible[1..]);
    timer.round("output".into());

    assert_eq!(solvable.len(), 1679072);
    info!("analyzed {total_moves} moves and probes");
    for (desc, dur) in timer.descriptions().zip(timer.durations()) {
        info!("{desc:>15}: {dur:>12?}");
    }
    info!("          total: {:>12?}", timer.total());
    solvable
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layer `pegs` of the growth phase computed the slow, obvious way: `Vec` BFS
    /// with sort+dedup, pruning the sources of each round as `grow` does.
    fn pruned_layer_by_vec(pegs: usize) -> Vec<Board> {
        let mut layer = vec![Board::solved()];
        for _ in 1..pegs {
            layer.retain(|&b| growth_survives_pagoda(b));
            let mut next = Board::possible_reverse_moves(&layer);
            Board::normalize_all(&mut next);
            next.sort_unstable();
            next.dedup();
            layer = next;
        }
        layer
    }

    /// A pool with layers `1..=top` grown, as the solver grows them.
    fn grown_pool(top: usize) -> KeySetPool {
        let pool = KeySetPool::new(top);
        {
            let l1 = pool.layer(1);
            l1.set_at(l1.index(Board::solved().to_compressed_repr()));
        }
        for pegs in 1..top {
            grow(pool.layer(pegs), pool.layer(pegs + 1));
        }
        pool
    }

    fn boards_of(layer: LayerView<'_>) -> Vec<Board> {
        layer
            .keys()
            .map(|(_, key)| Board::from_compressed_repr(key))
            .collect()
    }

    #[test]
    fn ring_returns_each_item_exactly_n_pushes_later() {
        let mut ring = Ring::<usize, 4>::new(usize::MAX);
        let mut returned = Vec::new();
        for i in 0..11 {
            if let Some(x) = ring.push(i) {
                assert_eq!(x, i - 4);
                returned.push(x);
            }
        }
        returned.extend(ring.drain());
        assert_eq!(returned, (0..11).collect::<Vec<_>>());
        assert_eq!(ring.pushed(), 11);
        // fewer pushes than the distance: nothing returned, everything drained
        let mut short = Ring::<usize, 4>::new(0);
        assert!(short.push(7).is_none());
        assert_eq!(short.drain().collect::<Vec<_>>(), vec![7]);
        assert!(Ring::<usize, 4>::new(0).drain().next().is_none());
    }

    /// `grow` has to produce exactly the reverse-move closure of its pruned source,
    /// which a ring off-by-one or a scan boundary error would silently truncate.
    #[test]
    fn grow_matches_vec_bfs() {
        // 10 layers: thousands of boards over several scan tasks, and pagoda pruning
        // starts to bite
        let pool = grown_pool(10);
        for pegs in 2..=10 {
            let got = boards_of(pool.layer(pegs));
            assert!(got.is_sorted());
            assert_eq!(
                got,
                pruned_layer_by_vec(pegs),
                "layer {pegs} differs from the reference"
            );
        }
    }

    /// The bridge's one-sided probing must mark exactly the boards the two-sided
    /// relation does (every pair probed from both sides). Only the real middle layer
    /// can be used: the relation connects a board to the successors of its inverse,
    /// which land in the same layer only when the inverse is one peg heavier.
    ///
    /// Checked on a sample against the brute-force relation, since the reference
    /// costs a hash lookup per move of 2.5M boards, plus the known total.
    #[test]
    fn bridge_matches_two_sided_probe() {
        let mut pool = grown_pool(MID);
        let layer = pool.layer(MID);
        let boards = boards_of(layer);
        let set: std::collections::HashSet<Board> = boards.iter().copied().collect();
        let related = |b: &Board| {
            Board::possible_moves(&[b.inverse()])
                .into_iter()
                .any(|c| set.contains(&c.normalize()))
        };

        bridge(layer, pool.scratch());
        let marked = pool.scratch();
        let (mut yes, mut no) = (0, 0);
        for b in boards.iter().step_by(97) {
            let want = related(b);
            assert_eq!(marked.test(b.to_compressed_repr()), want, "{b:?}");
            if want { yes += 1 } else { no += 1 }
        }
        assert!(yes > 0 && no > 0, "sample must exercise both outcomes");
        assert_eq!(pool.drain_scratch().len(), 230_230);
    }

    /// `shrink` must emit exactly the successors of its sources that the layer
    /// holds, each once, however many sources lead to it.
    #[test]
    fn shrink_emits_each_contained_successor_once() {
        let pool = grown_pool(9);
        // every other board of layer 9 as the source: enough sources share successors
        // that duplicates are common, and enough are missing that some successors in
        // layer 8 are not reached at all
        let src: Vec<Board> = boards_of(pool.layer(9)).into_iter().step_by(2).collect();
        let layer8: std::collections::HashSet<Board> =
            boards_of(pool.layer(8)).into_iter().collect();
        let mut succ = Board::possible_moves(&src);
        Board::normalize_all(&mut succ);
        let raw = succ.len();
        succ.sort_unstable();
        succ.dedup();
        assert!(
            succ.len() < raw,
            "no duplicates to test exactly-once emission on"
        );
        let want: Vec<Board> = succ.into_iter().filter(|b| layer8.contains(b)).collect();
        assert!(!want.is_empty() && want.len() < layer8.len());

        let (mut got, probes) = shrink(&src, pool.layer(8));
        assert_eq!(probes, raw);
        got.sort_unstable();
        assert_eq!(got, want);
        // and the hits were taken out of the layer
        let left = boards_of(pool.layer(8));
        assert_eq!(left.len(), layer8.len() - want.len());
        assert!(left.iter().all(|b| !want.contains(b)));
    }

    #[test]
    fn collect_with_inverse_pairs_every_board() {
        let layers = vec![
            vec![Board::solved()],
            Vec::new(),
            boards_of(grown_pool(5).layer(5)),
        ];
        let got = collect_with_inverse(&layers);
        let want: Vec<Board> = layers
            .iter()
            .flatten()
            .flat_map(|b| [*b, b.inverse().normalize()])
            .collect();
        assert_eq!(got, want);
    }
}
