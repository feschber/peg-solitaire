//! Which moves every winning continuation from a position has to make.
//!
//! The board-level question - "every winning line passes through this position" - is the
//! weaker one, and it is not what a player needs to be told. The edge-level question is:
//! *you have committed to what you have played; here is a move you will have to make later,
//! whatever else you do.* That is a dominator in the move graph rather than the board graph.
//!
//! No dominator tree is needed for it. Writing `count(x)` for the number of winning lines
//! from `x` - which [`crate::all_unique_paths`] already computes for every feasible board -
//! and `forward(u)` for the number of lines from the current board to `u`, every winning line
//! through the edge `(u, v)` splits uniquely into a prefix reaching `u`, the edge, and a
//! suffix from `v`. So
//!
//! ```text
//!     lines through (u, v)  =  forward(u) * count(v)
//! ```
//!
//! and the edge is forced exactly when that equals `count(current)`, i.e. when *no* winning
//! line avoids it. The split is a bijection because every move removes a peg, so peg count
//! strictly decreases along a line and no edge can be traversed twice - in a graph where a
//! path could reuse an edge the product would overcount.
//!
//! Everything here works in the *normalized* graph, the same one `all_unique_paths` counts
//! in. That is sound because a board's legal moves are the symmetry images of its
//! representative's, so the multiset of normalized successors is the same either way, which
//! is why the move-sequence count over the quotient reproduces the published figure for the
//! un-normalized game.

use crate::unique_solutions::move_at;
use crate::{Board, Dir, HashMap, HashSet, Move};

/// A transition every winning continuation has to make.
///
/// `board` is itself on every winning line - if the step out of it is unavoidable then so is
/// arriving there - so the pairing is unambiguous despite the quotient.
///
/// `realizations` is why this is a transition rather than flatly a move. A board can offer
/// two *different* moves whose successors are symmetric, and so identical after
/// normalization; the player must make one of them but is free to choose which. So:
///
/// - `realizations == 1` - exactly this move is forced, `mov` names it.
/// - `realizations > 1` - the step is forced and `mov` is one of the `realizations`
///   symmetric moves that take it, any of which will do.
///
/// Testing each parallel move separately would find neither forced, since each carries only
/// its share of the lines - which is precisely the bug the brute-force test caught.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForcedMove {
    /// normalized board the step is taken from
    pub board: Board,
    /// a move that takes it; the only one when `realizations == 1`
    pub mov: Move,
    /// normalized board it leads to
    pub next: Board,
    /// how many distinct moves from `board` realize this step
    pub realizations: u32,
}

/// What a legal move does to the player's chances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// every winning line makes this move *now*
    Required,
    /// at least one winning line makes it and at least one does not
    Optional,
    /// no winning line makes it
    Losing,
}

/// Classifies the moves legal right now. Cheap - one lookup per move, no traversal.
pub fn classify_moves(
    from: Board,
    feasible: &HashSet<Board>,
    counts: &HashMap<Board, u64>,
) -> Vec<(Move, Verdict)> {
    let syms = from.symmetries();
    let mut moves = Vec::new();
    let mut winning = 0usize;
    for dir in Dir::enumerate() {
        for idx in from.mov_pattern_mask(dir) {
            let next = Board::normalize_after_move(&syms, idx, dir);
            let lines = if feasible.contains(&next) {
                counts.get(&next).copied().unwrap_or(0)
            } else {
                0
            };
            if lines > 0 {
                winning += 1;
            }
            moves.push((move_at(idx, dir), lines));
        }
    }
    moves
        .into_iter()
        .map(|(mov, lines)| {
            let verdict = match (lines, winning) {
                (0, _) => Verdict::Losing,
                (_, 1) => Verdict::Required,
                _ => Verdict::Optional,
            };
            (mov, verdict)
        })
        .collect()
}

/// Every move edge that all winning continuations from `from` must traverse, in play order.
///
/// Returns empty when `from` is already lost, since then nothing is forced - there is no
/// winning line to constrain. Note a *board* can be unavoidable without any single edge into
/// it being so: from the opening position all four first moves are symmetric and so lead to
/// one normalized successor, which every line therefore visits, while each individual edge
/// carries only a quarter of the lines.
pub fn forced_moves(
    from: Board,
    feasible: &HashSet<Board>,
    counts: &HashMap<Board, u64>,
) -> Vec<ForcedMove> {
    let start = from.normalize();
    let total = counts.get(&start).copied().unwrap_or(0);
    if total == 0 {
        return Vec::new();
    }

    // forward[x] = winning lines from `start` that reach x. Filled layer by layer downwards,
    // so a board's own total is complete before any edge out of it is examined - a move
    // removes exactly one peg, so every predecessor of a board sits in the layer above it.
    let mut forward: HashMap<Board, u64> = HashMap::default();
    forward.insert(start, 1);
    let mut layers: Vec<Vec<Board>> = vec![Vec::new(); start.count_pegs() + 1];
    layers[start.count_pegs()].push(start);

    let mut forced = Vec::new();
    for pegs in (2..=start.count_pegs()).rev() {
        for u in std::mem::take(&mut layers[pegs]) {
            let reaching = forward[&u];
            let syms = u.symmetries();

            // Group the winning moves by their normalized successor first. Moves that land on
            // the same successor have to be counted together or none of them looks forced -
            // see `realizations`. At most 76 moves from a board, so a linear scan beats a map.
            let mut steps: Vec<(Board, Move, u32)> = Vec::new();
            for dir in Dir::enumerate() {
                for idx in u.mov_pattern_mask(dir) {
                    let v = Board::normalize_after_move(&syms, idx, dir);
                    if !feasible.contains(&v) || counts.get(&v).copied().unwrap_or(0) == 0 {
                        // a dead end: no winning line uses it, so it is neither forced nor a
                        // prefix for anything beyond it
                        continue;
                    }
                    match steps.iter_mut().find(|(seen, _, _)| *seen == v) {
                        Some((_, _, realizations)) => *realizations += 1,
                        None => steps.push((v, move_at(idx, dir), 1)),
                    }
                }
            }

            for (v, mov, realizations) in steps {
                let lines_from_v = counts[&v];
                // `checked_mul` rather than a wider type: the product counts a subset of the
                // lines from `start`, so it cannot really exceed `total`, and treating an
                // overflow as "not forced" is right for any product that could.
                let through = u64::from(realizations)
                    .checked_mul(reaching)
                    .and_then(|n| n.checked_mul(lines_from_v));
                if through == Some(total) {
                    forced.push(ForcedMove {
                        board: u,
                        mov,
                        next: v,
                        realizations,
                    });
                }

                // every parallel move contributes its own prefix count
                let arriving = u64::from(realizations) * reaching;
                if let Some(existing) = forward.get_mut(&v) {
                    *existing += arriving;
                } else {
                    forward.insert(v, arriving);
                    layers[pegs - 1].push(v);
                }
            }
        }
    }
    forced
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{all_unique_paths, calculate_feasible_set};

    /// Every winning line from `board`, as the set of (board, move-target) edges it uses.
    ///
    /// Brute force, so only usable a few pegs from the end - which is the point: it owes
    /// nothing to the counting identity `forced_moves` rests on, so agreeing with it is real
    /// evidence rather than a restatement.
    fn winning_lines(board: Board, feasible: &HashSet<Board>) -> Vec<Vec<(Board, Board)>> {
        if board.is_solved() {
            return vec![Vec::new()];
        }
        let syms = board.symmetries();
        let mut lines = Vec::new();
        for dir in Dir::enumerate() {
            for idx in board.mov_pattern_mask(dir) {
                let next = Board::normalize_after_move(&syms, idx, dir);
                if !feasible.contains(&next) {
                    continue;
                }
                for mut tail in winning_lines(next, feasible) {
                    tail.insert(0, (board, next));
                    lines.push(tail);
                }
            }
        }
        lines
    }

    #[test]
    fn forced_moves_match_brute_force_near_the_end() {
        let feasible = calculate_feasible_set(None);
        let counts = all_unique_paths(feasible.clone(), None);
        let set: HashSet<Board> = feasible.iter().copied().collect();

        // a handful of winnable boards shallow enough to enumerate exhaustively
        let mut checked = 0usize;
        let mut with_forced = 0usize;
        for pegs in [3usize, 4, 5, 6] {
            let boards: Vec<Board> = feasible
                .iter()
                .copied()
                .filter(|b| b.count_pegs() == pegs)
                .filter(|b| counts.get(b).copied().unwrap_or(0) > 0)
                .take(40)
                .collect();
            for board in boards {
                let lines = winning_lines(board, &set);
                assert!(!lines.is_empty(), "counts claim {board:?} is winnable");
                assert_eq!(
                    lines.len() as u64, counts[&board],
                    "brute force and the DP disagree on the line count for {board:?}"
                );

                // an edge is forced iff it appears in every enumerated line
                let mut expected: Vec<(Board, Board)> = lines[0].clone();
                expected.retain(|edge| lines.iter().all(|line| line.contains(edge)));
                expected.sort_unstable();

                let mut actual: Vec<(Board, Board)> = forced_moves(board, &set, &counts)
                    .into_iter()
                    .map(|f| (f.board, f.next))
                    .collect();
                actual.sort_unstable();
                actual.dedup();

                assert_eq!(actual, expected, "forced edges differ for {board:?}");
                checked += 1;
                with_forced += usize::from(!expected.is_empty());
            }
        }
        assert!(checked > 50, "only {checked} boards exercised");
        assert!(with_forced > 0, "no board had any forced move - test proves nothing");
    }

    /// The last jump of a won game is unavoidable, and nothing else is legal by then.
    #[test]
    fn the_final_move_is_required() {
        let feasible = calculate_feasible_set(None);
        let counts = all_unique_paths(feasible.clone(), None);
        let set: HashSet<Board> = feasible.iter().copied().collect();

        let two_pegs = feasible
            .iter()
            .copied()
            .find(|b| b.count_pegs() == 2 && counts.get(b).copied().unwrap_or(0) > 0)
            .expect("a winnable two-peg board must exist");

        let verdicts = classify_moves(two_pegs, &set, &counts);
        let required = verdicts.iter().filter(|(_, v)| *v == Verdict::Required).count();
        assert_eq!(required, 1, "exactly one move can finish a won two-peg board");
        assert_eq!(forced_moves(two_pegs, &set, &counts).len(), 1);
    }
}
