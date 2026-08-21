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

/// A jump every winning continuation has to make, named in the player's own frame.
///
/// [`ForcedMove`]'s `realizations` exists because the normalized graph has parallel edges;
/// this does not, because in the un-normalized graph two distinct moves from a board always
/// leave distinct boards - they clear and fill different slots. So a forced step here names
/// exactly one jump at literal board coordinates, which is what it takes to *draw* the hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForcedJump {
    /// board the jump is made from, in the same frame as the board passed in
    pub board: Board,
    /// the jump; no other move from `board` will do
    pub mov: Move,
    /// board it leads to
    pub next: Board,
}

/// Every jump all winning continuations from `from` must make, in play order, in `from`'s frame.
///
/// The un-normalized counterpart of [`forced_moves`], and the one a hint wants. Working in the
/// quotient is fine for *counting* but not for pointing at a square: `forced_moves` reports the
/// forced step out of the opening as one move with four realizations, when what is true of the
/// real game is that the player picks freely among four symmetric first moves. Here that step
/// simply is not forced, which is the honest answer.
///
/// No path counts are multiplied, because the identity in the module docs collapses. Write
/// `S(p)` for the boards with `p` pegs that are reachable from `from` through still-winnable
/// positions. Every winning line passes through exactly one board per peg count - a move
/// removes exactly one peg - so
///
/// ```text
///     total  =  sum over u in S(p) of  forward(u) * count(u)
/// ```
///
/// and `forward(u) * count(v) == total` therefore needs both factors to be maximal at once:
/// `count(v) == count(u)`, i.e. `v` is `u`'s *only* winning move, and `u` the only member of
/// `S(p)`. So a forced jump is exactly **a peg count at which the reachable set has collapsed
/// to a single board, from which a single move wins**. Both are decidable from reachability
/// alone, which is why this needs only the winnable predicate out of `counts` and never its
/// values - no products, and no overflow to guard.
///
/// That also says what the hint means, and it is worth knowing before trusting it: a forced
/// jump is never a hidden constraint on an otherwise open game, it is a stretch of the game
/// that has already funnelled.
///
/// `counts` is used only as "can this board still win", and is symmetry-invariant - it counts
/// winning move sequences, and a symmetry is a bijection on those - so the normalized map
/// answers for un-normalized boards through one `normalize` call.
///
/// The traversal is bounded by the boards reachable from `from` that can still win, so it is
/// widest at the opening and collapses quickly as pegs come off. Returns empty when `from` is
/// already lost - nothing is forced when nothing wins.
pub fn forced_jumps(from: Board, counts: &HashMap<Board, u64>) -> Vec<ForcedJump> {
    let winnable = |board: &Board| counts.get(&board.normalize()).copied().unwrap_or(0) > 0;
    if !winnable(&from) {
        return Vec::new();
    }

    // `layers[p]` accumulates S(p). Filled downwards, so a layer is complete before it is
    // judged: every predecessor of a board sits in the layer above it.
    let mut layers: Vec<Vec<Board>> = vec![Vec::new(); from.count_pegs() + 1];
    layers[from.count_pegs()].push(from);
    let mut seen: HashSet<Board> = HashSet::default();
    seen.insert(from);

    let mut forced = Vec::new();
    for pegs in (2..=from.count_pegs()).rev() {
        let layer = std::mem::take(&mut layers[pegs]);
        // only a layer that has narrowed to one board can host a forced jump, and then only
        // if that board has exactly one winning move - but the sweep has to continue either
        // way, since a later layer may still collapse
        let sole = if layer.len() == 1 {
            layer.first()
        } else {
            None
        };
        let mut only: Option<ForcedJump> = None;
        let mut candidates = 0usize;

        for u in &layer {
            for dir in Dir::enumerate() {
                for idx in u.mov_pattern_mask(dir) {
                    let mov = move_at(idx, dir);
                    let v = u.mov(mov);
                    if !winnable(&v) {
                        // a dead end: no winning line uses it, so it is neither forced nor a
                        // prefix for anything beyond it
                        continue;
                    }
                    if sole == Some(u) {
                        candidates += 1;
                        only = Some(ForcedJump {
                            board: *u,
                            mov,
                            next: v,
                        });
                    }
                    if seen.insert(v) {
                        layers[pegs - 1].push(v);
                    }
                }
            }
        }

        if candidates == 1 {
            forced.extend(only);
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
                    lines.len() as u64,
                    counts[&board],
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
        assert!(
            with_forced > 0,
            "no board had any forced move - test proves nothing"
        );
    }

    /// Every winning line from `board` in the un-normalized game, as its list of jumps.
    fn winning_jumps(board: Board, feasible: &HashSet<Board>) -> Vec<Vec<(Board, Move)>> {
        if board.is_solved() {
            return vec![Vec::new()];
        }
        let mut lines = Vec::new();
        for dir in Dir::enumerate() {
            for idx in board.mov_pattern_mask(dir) {
                let mov = move_at(idx, dir);
                let next = board.mov(mov);
                if !feasible.contains(&next.normalize()) {
                    continue;
                }
                for mut tail in winning_jumps(next, feasible) {
                    tail.insert(0, (board, mov));
                    lines.push(tail);
                }
            }
        }
        lines
    }

    /// Same shape of check as [`forced_moves_match_brute_force_near_the_end`], one frame down:
    /// the enumeration here never normalizes, so it is a direct statement about the game the
    /// player is actually playing.
    #[test]
    fn forced_jumps_match_brute_force_near_the_end() {
        let feasible = calculate_feasible_set(None);
        let counts = all_unique_paths(feasible.clone(), None);
        let set: HashSet<Board> = feasible.iter().copied().collect();

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
                let lines = winning_jumps(board, &set);
                assert_eq!(
                    lines.len() as u64,
                    counts[&board],
                    "the move-sequence count must be the un-normalized line count for {board:?}"
                );

                let mut expected: Vec<(Board, Move)> = lines[0].clone();
                expected.retain(|jump| lines.iter().all(|line| line.contains(jump)));
                expected.sort_unstable();

                let mut actual: Vec<(Board, Move)> = forced_jumps(board, &counts)
                    .into_iter()
                    .map(|f| (f.board, f.mov))
                    .collect();
                actual.sort_unstable();

                assert_eq!(actual, expected, "forced jumps differ for {board:?}");
                checked += 1;
                with_forced += usize::from(!expected.is_empty());
            }
        }
        assert!(checked > 50, "only {checked} boards exercised");
        assert!(
            with_forced > 0,
            "no board had any forced jump - test proves nothing"
        );
    }

    /// The opening's four first moves are symmetric images of each other, so the player really
    /// does have a choice - and unlike [`forced_moves`], the un-normalized answer says so.
    #[test]
    fn nothing_is_forced_out_of_the_opening() {
        let feasible = calculate_feasible_set(None);
        let counts = all_unique_paths(feasible.clone(), None);
        let set: HashSet<Board> = feasible.iter().copied().collect();

        let start = Board::default();
        assert!(
            forced_moves(start, &set, &counts)
                .iter()
                .any(|f| f.board == start.normalize() && f.realizations == 4),
            "the quotient reports the first move as one forced step with four realizations"
        );
        assert!(
            !forced_jumps(start, &counts)
                .iter()
                .any(|f| f.board == start),
            "no single first jump is forced in the game as played"
        );
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
        let required = verdicts
            .iter()
            .filter(|(_, v)| *v == Verdict::Required)
            .count();
        assert_eq!(
            required, 1,
            "exactly one move can finish a won two-peg board"
        );
        assert_eq!(forced_moves(two_pegs, &set, &counts).len(), 1);
    }
}
