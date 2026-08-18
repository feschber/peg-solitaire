//! Scores candidate pruning predicates for `calculate_feasible_set` against
//! ground truth, so an idea can be judged before it is wired into the hot path.
//!
//! A predicate claims "this board cannot reach `solved`". Using one to prune is
//! sound only if it never fires on a board that actually can, so the numbers that
//! matter are *false prunes* (must be exactly zero), the share of genuine
//! negatives caught, and the per-board cost. All three are reported.
//!
//! # Which questions are even answerable
//!
//! The growth phase builds its boards by legal reverse moves from `solved`, so
//! every board it produces is move-connected to `solved`. Therefore **any**
//! move-invariant - the GF(4) colouring, the type-class parity counts that
//! `Board::type_masks` sets up, Conway's rule of three - takes the same value on
//! all of them and prunes all or nothing. Only one-directional arguments can say
//! anything here.
//!
//! That is also why the pagoda filter in `feasible.rs` is applied to `b.inverse()`
//! rather than `b`. By the complement duality (a move `A -> B` implies a move
//! `complement(B) -> complement(A)`, and `complement(solved)` is the start
//! position), "`b` is reachable from the start" is exactly "`inverse(b)` can reach
//! `solved`" - a question the construction does not already answer.
//!
//! # Results so far
//!
//! ```text
//!                                 growth round      shrink round
//!   candidates                       2,499,905         3,163,355
//!   in the final answer                230,230           230,230
//!
//!   pagoda (in use today)               20.0%             21.0%     4.9 ns/board
//!   reachable-support closure            4.3%              5.3%     7.7 ns/board
//!     + stuck-peg scan                  +0.1pt            +0.0pt   18.4 ns/board
//!   both together                       22.4%             23.9%
//!   Fibonacci pagoda family              0.0%              0.0%
//! ```
//!
//! Two negatives worth not re-deriving:
//!
//! * The **Fibonacci/golden-ratio pagodas** - the classical strong ones, where
//!   `W[k] + W[k+1] == W[k+2]` meets the pagoda condition with equality - catch
//!   nothing here. Their weights are all positive, so a 16-peg board always
//!   outweighs the one-peg target and the test never fires. Pruning a *dense*
//!   board needs negative weights, which is why the weighting in `pagoda.rs` has
//!   them.
//! * The **reachable-support closure** is sound and genuinely independent of
//!   pagoda, but weak: it adds 2.4 points in the growth phase, and wiring it into
//!   the growth filter measured **+7.6% wall time** - `par_filter` evaluates its
//!   predicate twice, and at 7.7 ns against pagoda's 4.9 ns the extra pruning does
//!   not repay the scan. Measured, then reverted.
//!
//! Run with `cargo run --release --example score_predicates`.

use rayon::prelude::*;
use solitaire_solver::{Board, Dir};

const CENTER: u32 = 3 * Board::REPR as u32 + 3;

/// The weighting from `solitaire-solver/src/pagoda.rs`, duplicated because that
/// module is `pub(crate)`. The baseline every candidate has to beat.
#[rustfmt::skip]
const PAGODA: [i64; 64] = [
     0,  0, -2,  0, -2,  0,  0,  0,
     0,  0,  2,  2,  2,  0,  0,  0,
    -2,  2,  0,  2,  0,  2, -2,  0,
     0,  2,  2,  3,  2,  2,  0,  0,
    -2,  2,  0,  2,  0,  2, -2,  0,
     0,  0,  2,  2,  2,  0,  0,  0,
     0,  0, -2,  0, -2,  0,  0,  0,
     0,  0,  0,  0,  0,  0,  0,  0,
];

fn weigh(w: &[i64; 64], b: Board) -> i64 {
    let mut bits = b.0;
    let mut sum = 0;
    while bits != 0 {
        sum += w[bits.trailing_zeros() as usize];
        bits &= bits - 1;
    }
    sum
}

struct Geometry {
    east: u64,
    west: u64,
    south: u64,
    north: u64,
    /// orthogonal, on-board neighbours of each cell
    neighbours: [u64; 64],
}

impl Geometry {
    fn new() -> Self {
        let full = Board::full().0;
        let mut neighbours = [0u64; 64];
        for idx in 0..64u32 {
            if full >> idx & 1 == 0 {
                continue;
            }
            let (r, c) = (idx as i32 / 8, idx as i32 % 8);
            let mut n = 0u64;
            for (dr, dc) in [(0i32, 1i32), (0, -1), (1, 0), (-1, 0)] {
                let (nr, nc) = (r + dr, c + dc);
                if (0..7).contains(&nr) && (0..7).contains(&nc) {
                    let ni = (nr * 8 + nc) as u32;
                    if full >> ni & 1 == 1 {
                        n |= 1 << ni;
                    }
                }
            }
            neighbours[idx as usize] = n;
        }
        let m = |d| Board::empty().movable_positions(d).0;
        Self {
            east: m(Dir::East),
            west: m(Dir::West),
            south: m(Dir::South),
            north: m(Dir::North),
            neighbours,
        }
    }

    /// Every cell that could ever hold a peg from `b` onward.
    ///
    /// A peg can only appear at `x + 2d` by jumping from `x` over `x + d`, so the
    /// occupied set closed under "`x` and `x + d` present implies `x + 2d`
    /// present" contains the occupied set of every reachable position. Cells
    /// outside it are unreachable for the rest of the game. Deliberately
    /// over-approximate - it ignores that jumping consumes the pegs it needs -
    /// which is what keeps it to a few bitwise rounds.
    fn reachable_support(&self, b: Board) -> u64 {
        const R: usize = Board::REPR as usize;
        let mut r = b.0;
        loop {
            let mut next = r;
            next |= (r & (r >> 1) & self.east) << 2;
            next |= (r & (r << 1) & self.west) >> 2;
            next |= (r & (r >> R) & self.south) << (2 * R);
            next |= (r & (r << R) & self.north) >> (2 * R);
            if next == r {
                return r;
            }
            r = next;
        }
    }

    /// Some peg can never take part in a move again: every move that removes a peg
    /// needs a peg on an orthogonal neighbour, and here no neighbour can ever hold
    /// one. Fatal unless it is the lone peg already sitting on the centre.
    fn has_stuck_peg(&self, b: Board) -> bool {
        let support = self.reachable_support(b);
        let mut bits = b.0;
        while bits != 0 {
            let p = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            if self.neighbours[p] & support == 0 && !(b.count_pegs() == 1 && p as u32 == CENTER) {
                return true;
            }
        }
        false
    }
}

/// A weighting is a valid pagoda iff `w(pos) + w(mid) >= w(target)` for every
/// geometrically possible move - checked exhaustively, independent of any board.
/// This is the soundness proof for a candidate; false prunes then cannot happen.
fn is_valid_pagoda(w: &[i64; 64]) -> bool {
    let full = Board::full().0;
    for idx in 0..64u32 {
        if full >> idx & 1 == 0 {
            continue;
        }
        let (r, c) = (idx as i32 / 8, idx as i32 % 8);
        for (dr, dc) in [(0i32, 1i32), (0, -1), (1, 0), (-1, 0)] {
            let (mr, mc, tr, tc) = (r + dr, c + dc, r + 2 * dr, c + 2 * dc);
            if !(0..7).contains(&mr) || !(0..7).contains(&mc) {
                continue;
            }
            if !(0..7).contains(&tr) || !(0..7).contains(&tc) {
                continue;
            }
            let (mi, ti) = ((mr * 8 + mc) as usize, (tr * 8 + tc) as usize);
            if full >> mi & 1 == 0 || full >> ti & 1 == 0 {
                continue;
            }
            if w[idx as usize] + w[mi] < w[ti] {
                return false;
            }
        }
    }
    true
}

/// The classical strong pagodas: along either diagonal coordinate, a Fibonacci
/// weighting satisfies `W[k] + W[k+1] == W[k+2]`, meeting the pagoda condition
/// with equality one way and slack the other. Kept because "try the golden-ratio
/// pagodas" is the obvious next idea and the answer is no - see the module docs.
fn fibonacci_pagodas() -> Vec<[i64; 64]> {
    const LEN: usize = 20;
    let mut fib = [0i64; LEN];
    (fib[0], fib[1]) = (1, 1);
    for i in 2..LEN {
        fib[i] = fib[i - 1] + fib[i - 2];
    }
    let full = Board::full().0;
    let mut out = Vec::new();
    for diag in 0..2 {
        for sign in [1i32, -1] {
            for a in 0..LEN as i32 {
                let mut w = [0i64; 64];
                let mut ok = true;
                for idx in 0..64u32 {
                    if full >> idx & 1 == 0 {
                        continue;
                    }
                    let (r, c) = (idx as i32 / 8, idx as i32 % 8);
                    let d = if diag == 0 { r + c } else { r - c + 6 };
                    let k = a + sign * d;
                    if !(0..LEN as i32).contains(&k) {
                        ok = false;
                        break;
                    }
                    w[idx as usize] = fib[k as usize];
                }
                if ok && is_valid_pagoda(&w) {
                    out.push(w);
                }
            }
        }
    }
    out
}

type Pred = (&'static str, Box<dyn Fn(&Geometry, Board) -> bool + Sync>);

fn candidate_predicates() -> Vec<Pred> {
    let solved_weight = weigh(&PAGODA, Board::solved());
    vec![
        (
            "pagoda (in use today)",
            Box::new(move |_: &Geometry, b: Board| weigh(&PAGODA, b) < solved_weight),
        ),
        (
            "centre unreachable (closure)",
            Box::new(|g: &Geometry, b: Board| g.reachable_support(b) >> CENTER & 1 == 0),
        ),
        (
            "stuck peg (closure)",
            Box::new(|g: &Geometry, b: Board| g.has_stuck_peg(b)),
        ),
        (
            "pagoda OR closure",
            Box::new(move |g: &Geometry, b: Board| {
                weigh(&PAGODA, b) < solved_weight
                    || g.reachable_support(b) >> CENTER & 1 == 0
                    || g.has_stuck_peg(b)
            }),
        ),
    ]
}

fn dedup_normalized(mut v: Vec<Board>) -> Vec<Board> {
    Board::normalize_all(&mut v);
    v.par_sort_unstable();
    v.dedup();
    v
}

/// `dual` selects which side of the complement duality the predicate is asked
/// about: the growth phase must decide "is `b` reachable from the start", which is
/// "can `inverse(b)` reach solved"; the shrink phase asks about `b` directly.
fn score(
    label: &str,
    g: &Geometry,
    candidates: &[Board],
    truth: &std::collections::HashSet<u64>,
    dual: bool,
    preds: &[Pred],
) {
    let positives = candidates.iter().filter(|b| truth.contains(&b.0)).count();
    let negatives = candidates.len() - positives;
    println!(
        "\n{label}: {} distinct candidates, {positives} in the final answer, {negatives} negatives",
        candidates.len()
    );
    println!(
        "{:<32} {:>13} {:>18}",
        "predicate", "FALSE PRUNES", "negatives caught"
    );
    for (name, f) in preds {
        let (bad, caught) = candidates
            .par_iter()
            .map(|&b| {
                let subject = if dual { b.inverse() } else { b };
                match (f(g, subject), truth.contains(&b.0)) {
                    (true, true) => (1u64, 0u64),
                    (true, false) => (0, 1),
                    _ => (0, 0),
                }
            })
            .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        println!(
            "{:<32} {:>13} {:>10} ({:>5.1}%){}",
            name,
            bad,
            caught,
            100.0 * caught as f64 / negatives as f64,
            if bad > 0 { "   <-- UNSOUND" } else { "" }
        );
    }
}

fn main() {
    let g = Geometry::new();
    let preds = candidate_predicates();
    let solved_weight = weigh(&PAGODA, Board::solved());

    println!("ground truth: running the real solver...");
    let mut truth: Vec<Board> = solitaire_solver::calculate_feasible_set(None)
        .into_iter()
        .filter(|b| b.count_pegs() == 16)
        .collect();
    truth.par_sort_unstable();
    truth.dedup();
    // the flatten in `calculate_feasible_set` takes indices 0..=16, and the only
    // other route to a 16-peg entry would be inverting a 17-peg one, which is
    // outside that range - so this is exactly the post-intersection visited[16]
    println!("  true 16-peg set: {} boards (expect 230230)", truth.len());
    let truth_set: std::collections::HashSet<u64> = truth.iter().map(|b| b.0).collect();

    println!("regenerating the growth phase...");
    let mut level = vec![Board::solved()];
    let mut level15 = Vec::new();
    for round in 1..(Board::SLOTS - 1) / 2 {
        if round == (Board::SLOTS - 1) / 2 - 1 {
            level15 = level.clone();
        }
        let mut next = dedup_normalized(Board::possible_reverse_moves(&level));
        next.retain(|b| weigh(&PAGODA, b.inverse()) >= solved_weight);
        level = next;
    }
    println!(
        "  growth visited[16]: {} boards (expect 2046865)",
        level.len()
    );

    let growth_candidates = dedup_normalized(Board::possible_reverse_moves(&level15));
    score(
        "growth round",
        &g,
        &growth_candidates,
        &truth_set,
        true,
        &preds,
    );

    let v17: Vec<Board> = level.par_iter().map(|b| b.inverse().normalize()).collect();
    let shrink_candidates = dedup_normalized(Board::possible_moves(&v17));
    score(
        "shrink round",
        &g,
        &shrink_candidates,
        &truth_set,
        false,
        &preds,
    );

    println!("\nper-board cost (single-threaded, 1M boards):");
    for (name, f) in &preds {
        let t = std::time::Instant::now();
        let mut acc = 0u64;
        for &b in shrink_candidates.iter().take(1_000_000) {
            acc += f(&g, b) as u64;
        }
        let ns = t.elapsed().as_nanos() as f64 / 1e6;
        println!(
            "  {name:<32} {ns:>5.1} ns/board (~{:>3.0} cycles) [{acc}]",
            ns * 3.0
        );
    }

    // reported in aggregate because every member scores zero - see the module docs
    let fam = fibonacci_pagodas();
    let best = fam
        .iter()
        .map(|w| {
            let target = weigh(w, Board::solved());
            shrink_candidates
                .par_iter()
                .filter(|b| !truth_set.contains(&b.0) && weigh(w, **b) < target)
                .count()
        })
        .max()
        .unwrap_or(0);
    println!(
        "\n{} valid Fibonacci pagodas enumerated; the best catches {best} negatives \
         (all-positive weights cannot fall below a one-peg target)",
        fam.len()
    );
}
