//! What the GF(4) move invariant says about the feasible set - and what it buys.
//!
//! Two things come out of the same fact.
//!
//! **Geometry.** A move clears two pegs and fills one hole, so adjacent *raw* boards sit at
//! Hamming distance 3. That says nothing about whether other feasible boards are closer, and
//! nothing at all about the graph's nodes, which are *normalized* orbit representatives
//! rather than raw boards. Both are measured here.
//!
//! **Memory.** Put three consecutive collinear slots in GF(4) = {0, 1, w, w^2} with
//! `1 + w + w^2 = 0`, weighting slot `(r, c)` by `w^(r+c)`. A move touches three slots whose
//! exponents are consecutive, so it changes the sum by `w^k (1 + w + w^2) = 0` - and in
//! characteristic 2 clearing and filling are the same operation, so the sum is *invariant*
//! under every move. Same for `w^(r-c)`. Every feasible board is move-connected to the start,
//! so every feasible board carries the start's two invariant values.
//!
//! GF(4) is two bits, so that is four GF(2)-linear constraints on a 33-bit key. If they are
//! independent, four of the 33 bits are redundant - reconstructible from the other 29 - and a
//! 29-bit key fits in a `u32` where a 33-bit one cannot. That is the interesting number for
//! `keyset.rs` and for anything else storing keys by the million.
//!
//! The distance-1 result also follows immediately: two boards differing in one slot `s` have
//! invariants differing by `w^(r+c)` and `w^(r-c)`, both always non-zero, so they cannot both
//! carry the start's values. Proven rather than searched - but searched anyway below, because
//! a proof about the wrong board geometry is worth nothing.
//!
//! Measured over the full feasible set (1_679_072 normalized boards):
//!
//! - **The subspace is closed under everything the solver does**, proven exhaustively rather
//!   than sampled: all 38 move masks evaluate to zero (so every move preserves the invariant,
//!   by linearity, for every board any move sequence can reach), and all 8 symmetries map the
//!   affine subspace `start + ker f` into itself. That is what makes a packed key safe for the
//!   boards the solver stores *and later discards* - every key it holds comes from
//!   `normalize_after_move`, i.e. a move followed by a symmetry, and nothing else.
//! - The four functionals have rank **exactly 4**, so **29-bit keys suffice** and those *do*
//!   fit a `u32` where 33-bit keys do not. Reconstructible bit positions: 2, 3, 10, 11.
//! - Start and solved both carry invariants `[1, 0, 1, 0]` - they must agree, since one is
//!   reachable from the other - and **0 of 1_679_072 boards violate them**.
//! - The packing **round-trips on all 1_679_072 boards**, with no key exceeding 29 bits. The
//!   dropped bits are *not* constant - they are set in 60.97%, 64.70%, 52.69% and 54.00% of
//!   boards respectively. They are reconstructible, which is a different thing: in reduced row
//!   echelon form each row reads `b[pivot] ^ (parity of its non-pivot bits) = target`.
//! - **0 feasible pairs at Hamming distance 1**, as proven.
//! - Distance 2 is common: 5_864_442 pairs with equal peg count (one peg relocated) and
//!   7_971_522 with peg counts differing by 2, each unordered pair counted twice, so ~6.92M
//!   distinct pairs against 8.58M move edges. All of them fall on the 46 permitted slot
//!   pairs; a 50_000-board sweep of the other 482 pairs found none, confirming the mod-3
//!   derivation empirically rather than only algebraically.
//! - Adjacent *normalized* nodes are at distance 3 only **73.90%** of the time. The rest run
//!   over every odd distance up to 29 - odd because a move changes the peg count by one.
//!   Normalization, not the move, is what stretches them.
//!
//! Analysis only. Run with `cargo run --release --example hamming_neighbors`.

use rayon::prelude::*;
use solitaire_solver::{Board, HashSet, calculate_feasible_set};

/// A playable slot: its bit position in `Board`'s `u64`, and where it is on the cross.
#[derive(Clone, Copy)]
struct Slot {
    bit: u32,
    row: i32,
    col: i32,
}

/// The 33 slots in increasing bit order, which is also the order `to_compressed_repr`
/// gathers them, so slot index here equals bit index in the compressed key.
fn slots() -> Vec<Slot> {
    let mut slots = Vec::with_capacity(Board::SLOTS);
    for row in 0..7i32 {
        let cols: Vec<i32> = if matches!(row, 0 | 1 | 5 | 6) {
            (2..5).collect()
        } else {
            (0..7).collect()
        };
        for col in cols {
            // `Board::REPR` is 8, i.e. one byte per row
            slots.push(Slot { bit: (row * 8 + col) as u32, row, col });
        }
    }
    assert_eq!(slots.len(), Board::SLOTS);
    slots
}

/// `w^(k mod 3)` in GF(4), as two bits: `1 = 0b01`, `w = 0b10`, `w^2 = w + 1 = 0b11`.
///
/// Never zero, which is the whole reason distance 1 is impossible.
fn gf4_pow(exponent: i32) -> u8 {
    [0b01, 0b10, 0b11][exponent.rem_euclid(3) as usize]
}

/// The four GF(2) functionals, each a mask over `Board`'s `u64`.
///
/// Two invariants (`r+c` and `r-c`), each GF(4)-valued, each contributing two bits - so
/// `functional(board) = parity of popcount(board & mask)` for four masks.
fn functionals(slots: &[Slot]) -> [u64; 4] {
    let mut masks = [0u64; 4];
    for slot in slots {
        let sum = gf4_pow(slot.row + slot.col);
        let difference = gf4_pow(slot.row - slot.col);
        for (index, weight) in [(0, sum), (2, difference)] {
            for bit in 0..2 {
                if weight >> bit & 1 == 1 {
                    masks[index + bit] |= 1 << slot.bit;
                }
            }
        }
    }
    masks
}

fn evaluate(masks: &[u64; 4], board: u64) -> [u32; 4] {
    masks.map(|mask| (board & mask).count_ones() & 1)
}

/// Row-reduces the functionals over GF(2), returning the pivot bit positions - the bits a
/// stored key could drop and rebuild from the rest plus the known invariant values - along
/// with the reduced rows themselves.
///
/// The reduced rows matter to callers, not just the pivots: in reduced form each row is the
/// only one with a 1 in its own pivot column, which is what lets a single pass over the rows
/// correct a vector one pivot at a time. Row operations do not change the kernel, so the
/// reduced system describes exactly the same subspace.
fn redundant_bits(masks: &[u64; 4], targets: [u32; 4]) -> (Vec<u32>, [u64; 4], [u32; 4]) {
    let mut rows = masks.to_vec();
    let mut targets = targets;
    let mut pivots = Vec::new();
    let mut next = 0;
    for bit in 0..64 {
        // find a remaining row with this bit set, and eliminate it from all the others
        let Some(found) = (next..rows.len()).find(|&r| rows[r] >> bit & 1 == 1) else {
            continue;
        };
        rows.swap(next, found);
        targets.swap(next, found);
        for other in 0..rows.len() {
            if other != next && rows[other] >> bit & 1 == 1 {
                rows[other] ^= rows[next];
                // the constraint is affine, so whatever is done to a row must be done to its
                // target too, or the reduced system describes a different subspace
                targets[other] ^= targets[next];
            }
        }
        pivots.push(bit);
        next += 1;
        if next == rows.len() {
            break;
        }
    }
    (pivots, [rows[0], rows[1], rows[2], rows[3]], targets)
}

/// Every legal move's XOR mask: three consecutive collinear slots, horizontally or
/// vertically. Direction does not matter - a move and its reverse share a mask.
fn move_masks(slots: &[Slot]) -> Vec<u64> {
    let bit_of = |row: i32, col: i32| {
        slots
            .iter()
            .find(|s| s.row == row && s.col == col)
            .map(|s| 1u64 << s.bit)
    };
    let mut masks = Vec::new();
    for slot in slots {
        for (dr, dc) in [(0, 1), (1, 0)] {
            let mut triple = (0..3).map(|k| bit_of(slot.row + dr * k, slot.col + dc * k));
            if let Some(mask) = triple.try_fold(0u64, |acc, b| Some(acc | b?)) {
                masks.push(mask);
            }
        }
    }
    masks
}

/// A basis for `ker f`, one vector per non-pivot slot.
///
/// Each vector sets its own non-pivot bit and whichever pivot bits are needed to bring all
/// four functionals back to zero. That works one pivot at a time only against the *reduced*
/// rows, where each pivot column belongs to a single row; against the original masks a pivot
/// touches several rows at once and correcting one breaks another.
fn kernel_basis(
    masks: &[u64; 4],
    reduced: &[u64; 4],
    slots: &[Slot],
    pivots: &[u32],
) -> Vec<u64> {
    let mut basis = Vec::new();
    for slot in slots.iter().filter(|s| !pivots.contains(&s.bit)) {
        let mut vector = 1u64 << slot.bit;
        // fix up one pivot at a time; each pivot is the only one of the four able to
        // influence its own functional row, so a single sweep is enough
        for (row, &pivot) in pivots.iter().enumerate() {
            if (vector & reduced[row]).count_ones() & 1 == 1 {
                vector |= 1 << pivot;
            }
        }
        // checked against the original functionals, not the reduced ones - the whole point is
        // that the two describe the same kernel
        assert_eq!(evaluate(masks, vector), [0; 4], "kernel vector is not in the kernel");
        basis.push(vector);
    }
    basis
}

/// Drops the four redundant bits and puts them back.
///
/// This is the part that turns "rank 4, so four bits are redundant" into something usable.
/// Rank only proves a reconstruction *exists*; `verify` below is what shows this one is it.
///
/// Nothing here is about the pivot bits being constant - they are not. Bit 2 is set in `start`
/// and clear in `solved`. They are *determined*: in reduced row echelon form each row has its
/// pivot as its only pivot-column entry, so that row reads
/// `b[pivot] ^ (parity of the row's non-pivot bits) = target`, which solves for `b[pivot]`.
///
/// The pivot set is not unique either - any four columns whose 4x4 submatrix is invertible
/// would serve. These four are simply the ones elimination reaches first in bit order.
struct Packing {
    pivots: [u32; 4],
    rows: [u64; 4],
    targets: [u32; 4],
    pivot_mask: u64,
    /// the 29 surviving slot positions, ascending, so bit `i` of a packed key is slot
    /// `carried[i]`
    carried: Vec<u32>,
}

impl Packing {
    fn new(slots: &[Slot], pivots: &[u32], rows: [u64; 4], targets: [u32; 4]) -> Self {
        let pivot_mask = pivots.iter().fold(0u64, |acc, &b| acc | 1 << b);
        Self {
            pivots: [pivots[0], pivots[1], pivots[2], pivots[3]],
            rows,
            targets,
            pivot_mask,
            carried: slots
                .iter()
                .map(|s| s.bit)
                .filter(|b| pivot_mask >> b & 1 == 0)
                .collect(),
        }
    }

    fn pack(&self, board: u64) -> u32 {
        self.carried
            .iter()
            .enumerate()
            .fold(0u32, |key, (i, &bit)| key | (((board >> bit) & 1) as u32) << i)
    }

    fn unpack(&self, key: u32) -> u64 {
        let mut board = self
            .carried
            .iter()
            .enumerate()
            .fold(0u64, |b, (i, &bit)| b | u64::from(key >> i & 1) << bit);
        for row in 0..4 {
            let parity = (board & self.rows[row] & !self.pivot_mask).count_ones() & 1;
            if parity ^ self.targets[row] == 1 {
                board |= 1 << self.pivots[row];
            }
        }
        board
    }
}

/// `ways[i][j][state]` = number of ways to choose `j` of the slots from `i` onward whose
/// weights XOR to `state`, where `state` packs both invariants into 4 bits.
///
/// This is what makes the two compressions composable. Combinatorial ranking works because
/// the subsets with a given prefix can be counted in O(1) from a binomial table; the same
/// trick survives adding the invariant, because the invariant is a running XOR and so is just
/// extra state to count over. The table is `34 * 34 * 16` entries - a few hundred KiB - and a
/// rank costs one lookup per slot.
fn ways_table(slots: &[Slot], masks: &[u64; 4]) -> Vec<Vec<[u128; 16]>> {
    let n = slots.len();
    let mut ways = vec![vec![[0u128; 16]; n + 2]; n + 1];
    ways[n][0][0] = 1;
    for i in (0..n).rev() {
        // both invariants' contribution from this one slot, as a 4-bit state
        let weight = (0..4).fold(0usize, |acc, bit| {
            acc | ((masks[bit] >> slots[i].bit & 1) as usize) << bit
        });
        for j in 0..=n {
            for state in 0..16 {
                let skip = ways[i + 1][j][state];
                let take = if j == 0 {
                    0
                } else {
                    ways[i + 1][j - 1][state ^ weight]
                };
                ways[i][j][state] = skip + take;
            }
        }
    }
    ways
}

fn binomial(n: usize, k: usize) -> u128 {
    (0..k).fold(1u128, |acc, i| acc * (n - i) as u128 / (i + 1) as u128)
}

fn main() {
    env_logger::init();
    let slots = slots();
    let masks = functionals(&slots);
    let expected = evaluate(&masks, Board(Board::full().0 & !Board::solved().0).0);
    let (pivots, reduced, reduced_targets) = redundant_bits(&masks, expected);

    println!("== invariant structure ==");
    println!("  4 GF(2) functionals, rank {} over the 33 slots", pivots.len());
    println!(
        "  so {} of 33 bits are redundant -> {}-bit keys, which {} fit a u32",
        pivots.len(),
        Board::SLOTS - pivots.len(),
        if Board::SLOTS - pivots.len() <= 32 { "DO" } else { "do not" }
    );
    println!("  reconstructible bit positions: {pivots:?}");

    let start = Board(Board::full().0 & !Board::solved().0);
    println!("  start invariants: {:?}", evaluate(&masks, start.0));
    println!("  solved invariants: {:?}", evaluate(&masks, Board::solved().0));

    // ---- Is the subspace closed under everything the solver does to a board?
    //
    // This is what decides whether a packed key is safe, and it needs more than the feasible
    // set to answer: the solver stores boards it later discards, so "every *feasible* board
    // satisfies the invariant" is not enough. But every key it stores comes from
    // `normalize_after_move` - a move, then a symmetry - so closure under those two
    // operations covers everything it can ever hold, discarded or not. Both checks below are
    // exhaustive rather than sampled, which is the point: linearity makes that possible.
    println!("\n== closure under the solver's operations ==");

    // A move is an XOR with a 3-slot mask, and the functionals are GF(2)-linear, so a move
    // preserves them exactly when the mask itself evaluates to zero. Checking every mask is
    // a complete proof for every board any sequence of moves can reach.
    let masks_of_moves = move_masks(&slots);
    let bad_moves = masks_of_moves
        .iter()
        .filter(|&&m| evaluate(&masks, m) != [0; 4])
        .count();
    println!(
        "  {} move masks, {bad_moves} of them change the invariant (expected 0)",
        masks_of_moves.len()
    );

    // Symmetries are the subtle half. A reflection sends `r+c` to `r-c`, so it *swaps* the
    // two invariants rather than fixing them - individual invariant values are not generally
    // preserved. It works here only because start and solved both have I1 == I2, and that
    // pair happens to be fixed by the whole group. Verified rather than assumed: the affine
    // subspace is `start + ker f`, so a symmetry maps it into itself exactly when it keeps
    // `start` inside and sends every kernel basis vector back into the kernel.
    let basis = kernel_basis(&masks, &reduced, &slots, &pivots);
    println!("  kernel basis has {} vectors (expect 33 - {})", basis.len(), pivots.len());
    let mut broken = 0usize;
    for (index, image) in start.symmetries().iter().enumerate() {
        let offset_ok = evaluate(&masks, image.0) == expected;
        // a symmetry is a slot permutation, so applying it to a basis vector means applying
        // it to that vector read as a board
        let kernel_ok = basis
            .iter()
            .all(|&v| evaluate(&masks, Board(v).symmetries()[index].0) == [0; 4]);
        if !(offset_ok && kernel_ok) {
            broken += 1;
            println!("    symmetry {index}: offset_ok {offset_ok}, kernel_ok {kernel_ok}");
        }
    }
    println!("  symmetries mapping the subspace off itself: {broken} of 8 (expected 0)");
    println!(
        "  => a {}-bit packed key is {} for every board the solver stores",
        Board::SLOTS - pivots.len(),
        if bad_moves == 0 && broken == 0 { "SAFE" } else { "UNSAFE" }
    );

    // ---- Does this compose with the C(33, k) ranking `keyspace_footprint.rs` explored?
    //
    // The two constraints are of different kinds - one is four GF(2)-linear conditions, the
    // other is a fixed popcount - and neither implies the other, so they should compose. What
    // is not obvious is whether the invariant is *equidistributed* across the k-subsets; if it
    // is, the combined count is C(33, k) / 16, but that has to be counted rather than assumed,
    // especially at small k where there is little room for it to even out.
    println!("\n== combining with C(33, k) ranking ==");
    let target = expected
        .iter()
        .enumerate()
        .fold(0usize, |acc, (bit, &v)| acc | (v as usize) << bit);
    let ways = ways_table(&slots, &masks);
    let bits_for = |count: u128| {
        (0..64).find(|b| count <= 1u128 << b).unwrap_or(64)
    };

    println!("   k   C(33,k)      C(33,k)/16   invariant & popcount   bits  vs C(33,k)");
    let mut total = 0u128;
    for k in [8usize, 12, 16, 17, 20, 24] {
        let combined = ways[0][k][target];
        total += combined;
        println!(
            "  {k:2}  {:>12}  {:>12}  {:>12}  {:>8}  {:>5.2}x",
            binomial(slots.len(), k),
            binomial(slots.len(), k) / 16,
            combined,
            bits_for(combined),
            binomial(slots.len(), k) as f64 / combined.max(1) as f64,
        );
    }
    let all: u128 = (0..=slots.len()).map(|k| ways[0][k][target]).sum();
    println!(
        "  all popcounts: {all} boards satisfy the invariant (2^29 = {})",
        1u128 << 29
    );
    println!(
        "  worst single layer needs {} bits, against 29 for the invariant alone \
         and {} for ranking alone",
        (0..=slots.len()).map(|k| bits_for(ways[0][k][target])).max().unwrap(),
        (0..=slots.len()).map(|k| bits_for(binomial(slots.len(), k))).max().unwrap(),
    );
    let _ = total;

    println!("\ncalculating feasible set ...");
    let feasible = calculate_feasible_set(None);
    let set: HashSet<Board> = feasible.iter().copied().collect();
    println!("  {} normalized feasible boards", feasible.len());

    // Every feasible board must carry the start's invariants. If this fails, the geometry
    // above is wrong and nothing else printed here means anything.
    let violations = feasible
        .par_iter()
        .filter(|b| evaluate(&masks, b.0) != expected)
        .count();
    println!("  boards violating the invariant: {violations} (expected 0)");

    // ---- Does the packing actually round-trip? Rank says a reconstruction exists; this is
    // the only thing that shows the one above is correct.
    println!("\n== 29-bit packing ==");
    let packing = Packing::new(&slots, &pivots, reduced, reduced_targets);
    let (broken_roundtrip, oversized, pivot_ones) = feasible
        .par_iter()
        .map(|board| {
            let key = packing.pack(board.0);
            let mut set_pivots = [0usize; 4];
            for (i, &pivot) in packing.pivots.iter().enumerate() {
                set_pivots[i] = (board.0 >> pivot & 1) as usize;
            }
            (
                usize::from(packing.unpack(key) != board.0),
                usize::from(key >= 1 << 29),
                set_pivots,
            )
        })
        .reduce(
            || (0, 0, [0; 4]),
            |a, b| {
                let mut pivot_ones = a.2;
                for (x, y) in pivot_ones.iter_mut().zip(b.2) {
                    *x += y;
                }
                (a.0 + b.0, a.1 + b.1, pivot_ones)
            },
        );
    println!("  boards failing pack/unpack round-trip: {broken_roundtrip} (expected 0)");
    println!("  packed keys needing 30 bits or more:    {oversized} (expected 0)");
    // and the reason this works is *not* that the dropped bits are constant
    for (i, &pivot) in packing.pivots.iter().enumerate() {
        println!(
            "  dropped bit {pivot:2} is set in {:>9} of {} boards ({:5.2}%) - determined, not constant",
            pivot_ones[i],
            feasible.len(),
            100.0 * pivot_ones[i] as f64 / feasible.len() as f64
        );
    }

    // ---- Hamming distance 1: proven impossible, checked anyway.
    println!("\n== Hamming distance 1 ==");
    let d1 = feasible
        .par_iter()
        .map(|board| {
            slots
                .iter()
                .filter(|s| set.contains(&Board(board.0 ^ (1 << s.bit)).normalize()))
                .count()
        })
        .sum::<usize>();
    println!("  feasible pairs at distance 1: {d1} (proof says 0)");

    // ---- Hamming distance 2, over the pairs the invariant permits.
    // Both functionals must agree on the two slots, which forces same row and column
    // residues mod 3 - so only these pairs can possibly join two feasible boards.
    let candidates: Vec<(u32, u32)> = (0..slots.len())
        .flat_map(|a| (a + 1..slots.len()).map(move |b| (a, b)))
        .filter(|&(a, b)| {
            (slots[a].row - slots[b].row).rem_euclid(3) == 0
                && (slots[a].col - slots[b].col).rem_euclid(3) == 0
        })
        .map(|(a, b)| (slots[a].bit, slots[b].bit))
        .collect();
    println!("\n== Hamming distance 2 ==");
    println!(
        "  {} of {} slot pairs pass the invariant (same row and col mod 3)",
        candidates.len(),
        slots.len() * (slots.len() - 1) / 2
    );

    let (d2_same_pegs, d2_two_pegs) = feasible
        .par_iter()
        .map(|board| {
            let mut same = 0usize;
            let mut two = 0usize;
            for &(a, b) in &candidates {
                let other = Board(board.0 ^ (1 << a) ^ (1 << b));
                if set.contains(&other.normalize()) {
                    if other.count_pegs() == board.count_pegs() {
                        same += 1;
                    } else {
                        two += 1;
                    }
                }
            }
            (same, two)
        })
        .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    println!("  pairs at distance 2, equal peg count (one peg relocated): {d2_same_pegs}");
    println!("  pairs at distance 2, peg counts differing by 2:          {d2_two_pegs}");

    // Belt and braces: the mod-3 filter above is derived, so confirm on a sample that no
    // *unfiltered* pair is being missed. A hit here would mean the invariant is wrong.
    let sample = 50_000.min(feasible.len());
    let outside = feasible[..sample]
        .par_iter()
        .map(|board| {
            let mut hits = 0usize;
            for a in 0..slots.len() {
                for b in a + 1..slots.len() {
                    let filtered = (slots[a].row - slots[b].row).rem_euclid(3) == 0
                        && (slots[a].col - slots[b].col).rem_euclid(3) == 0;
                    if filtered {
                        continue;
                    }
                    let other = Board(board.0 ^ (1 << slots[a].bit) ^ (1 << slots[b].bit));
                    if set.contains(&other.normalize()) {
                        hits += 1;
                    }
                }
            }
            hits
        })
        .sum::<usize>();
    println!("  distance-2 hits outside the filter, over {sample} boards: {outside} (expected 0)");

    // ---- What the graph's edges actually look like, which is the layout-relevant number.
    // A move is distance 3 between raw boards, but the nodes are normalized, so the
    // representative of the successor need not be the one adjacent to this representative.
    println!("\n== distance between adjacent nodes (normalized) ==");
    let histogram = feasible
        .par_iter()
        .map(|board| {
            let mut counts = [0usize; 34];
            for mov in board.get_legal_moves() {
                let successor = board.mov(mov).normalize();
                if set.contains(&successor) {
                    counts[(board.0 ^ successor.0).count_ones() as usize] += 1;
                }
            }
            counts
        })
        .reduce(
            || [0usize; 34],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(b) {
                    *x += y;
                }
                a
            },
        );
    let edges: usize = histogram.iter().sum();
    println!("  {edges} edges (counting duplicates from distinct moves)");
    for (distance, &count) in histogram.iter().enumerate() {
        if count > 0 {
            println!(
                "    distance {distance:2}: {count:>9}  ({:5.2}%)",
                100.0 * count as f64 / edges as f64
            );
        }
    }
}
