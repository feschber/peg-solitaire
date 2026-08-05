use crate::Board;

/// a pagoda function: weights such that for every legal move (peg jumps from `pos`
/// over `mid` into `target`), `w(pos) + w(mid) >= w(target)`. This makes the total
/// weight of occupied cells non-increasing along any forward move sequence, so
/// `pagoda(board) < pagoda(Board::solved())` proves `board` can never reach
/// `solved` via forward play - an exact, correctness-preserving pruning check, not
/// a heuristic.
///
/// found by a symmetry-respecting search over the 7 orbits of the 33 cells under
/// the board's 8-fold symmetry (see `examples/find_pagoda.rs`), maximizing pruning
/// power against this solver's own ground-truth feasible set while requiring zero
/// false exclusions. About 6x stronger than the original single-weighting version
/// (~21% vs ~3.5% of growth-phase negatives pruned at the biggest round) - the
/// weighting search plateaued here even with a much wider integer range, so this
/// is close to the ceiling for this class of technique on this board.
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

/// `ROW_WEIGHTS[row][byte]` = sum of `PAGODA` weights for the set bits of `byte`,
/// treated as row `row`'s 8 cells (`Board::REPR == 8`, so each row is exactly one
/// byte of the underlying `u64`). Precomputing this turns `pagoda()` into 8 fixed
/// table lookups instead of one iteration per occupied cell (up to ~32 for these
/// boards) - the peg-count-scaling version measurably regressed generation time
/// once wired into the hot path, since its cost was comparable to the atomic
/// writes it was meant to let us skip.
const ROW_WEIGHTS: [[i64; 256]; 8] = {
    let mut tables = [[0i64; 256]; 8];
    let mut row = 0;
    while row < 8 {
        let mut byte = 0;
        while byte < 256 {
            let mut sum = 0i64;
            let mut bit = 0;
            while bit < 8 {
                if (byte >> bit) & 1 == 1 {
                    sum += PAGODA[row * 8 + bit];
                }
                bit += 1;
            }
            tables[row][byte] = sum;
            byte += 1;
        }
        row += 1;
    }
    tables
};

pub(crate) fn pagoda(board: Board) -> i64 {
    let bits = board.0;
    let mut sum = 0i64;
    let mut row = 0usize;
    while row < 8 {
        sum += ROW_WEIGHTS[row][((bits >> (row * 8)) & 0xFF) as usize];
        row += 1;
    }
    sum
}

#[test]
fn test_pagoda_matches_peg_by_peg_reference() {
    fn pagoda_reference(board: Board) -> i64 {
        board.into_iter().map(|i| PAGODA[i]).sum()
    }
    let samples = [Board::empty(), Board::full(), Board::solved(), Board::default()]
        .into_iter()
        .chain((0..100_000).map(|_| Board(rand::random::<u64>() & Board::full().0)));
    for board in samples {
        assert_eq!(pagoda(board), pagoda_reference(board), "mismatch for {board:?}");
    }
}

#[test]
fn test_pagoda_is_valid() {
    // exhaustively verify w(pos) + w(mid) >= w(target) for every geometrically
    // possible move on the board (independent of any particular peg placement) -
    // this is the actual soundness proof, not just a spot check. If this fails,
    // `pagoda()` could incorrectly exclude a genuinely reachable board.
    fn idx(y: crate::Idx, x: crate::Idx) -> usize {
        y as usize * Board::REPR as usize + x as usize
    }
    let mut checked = 0;
    for y in 0..Board::SIZE {
        for x in 0..Board::SIZE {
            if !Board::inbounds((y, x)) {
                continue;
            }
            for (dy, dx) in [(0i8, 1i8), (0, -1), (1, 0), (-1, 0)] {
                let mid = (y + dy, x + dx);
                let tgt = (y + 2 * dy, x + 2 * dx);
                if Board::inbounds(mid) && Board::inbounds(tgt) {
                    let (wp, wm, wt) = (PAGODA[idx(y, x)], PAGODA[mid.0 as usize * Board::REPR as usize + mid.1 as usize], PAGODA[tgt.0 as usize * Board::REPR as usize + tgt.1 as usize]);
                    assert!(
                        wp + wm >= wt,
                        "invalid pagoda weighting: move ({y},{x})->({},{}) violates w(pos)+w(mid)>=w(target): {wp}+{wm} < {wt}",
                        mid.0, mid.1
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 0);
}
