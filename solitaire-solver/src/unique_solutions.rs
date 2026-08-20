use crate::solution::SolutionMultiset;
use crate::dir::Dir;
use crate::par;
use crate::{Board, Move};
use crate::{HashMap, HashSet, Solution};
use std::collections::BTreeMap;
use std::num::NonZero;

/// we define two solutions as "equal" when the
///  multiset of steps is equivalent between them
///
/// Finds all *unique* solutions (by step-multiset) from `start` to any board in `goals`.
///
/// Uses BFS/DFS over the feasible graph, accumulating the multiset of steps along
/// each path.  When a goal is reached the current multiset is inserted into the
/// result set — duplicates collapse automatically.
pub fn all_unique_solutions(
    start: Board,
    feasible: impl Iterator<Item = Board>,
) -> std::collections::HashSet<SolutionMultiset> {
    log::info!("calculating unique solutions ....");
    let feasible: HashSet<Board> = feasible.collect();

    // Work-stack entry: (current_board, accumulated_multiset, hash of multiset)
    // Using a stack (DFS) keeps memory proportional to path depth;
    // swap for a VecDeque + pop_front if you prefer BFS.
    let mut stack: Vec<(Board, SolutionMultiset, MultisetHash)> = vec![(start, BTreeMap::new(), 0)];

    let mut unique_solutions: std::collections::HashSet<SolutionMultiset> =
        std::collections::HashSet::default();

    let mut visited: std::collections::HashSet<(Board, MultisetHash)> =
        std::collections::HashSet::new();
    println!();
    let mut zobrist = ZobristTable::default();
    visited.insert((start, 0));

    while let Some((board, multiset, hash)) = stack.pop() {
        if board.is_solved() {
            unique_solutions.insert(multiset);
            // Do NOT continue here if a goal board can still have outgoing
            // moves that lead to *other* goals; change to `continue` if goals
            // are always terminal.
            continue;
        }

        for mov in board.get_legal_moves() {
            let next_board = board.mov(mov);
            // Only follow edges that stay within the feasible set
            if !feasible.contains(&next_board.normalize()) {
                continue;
            }

            // Extend the multiset with this step
            let mut next_multiset = multiset.clone();

            let new_count = {
                let c = next_multiset.entry(mov).or_insert(0);
                *c += 1;
                *c
            };
            let next_hash = hash ^ zobrist.delta(&mov, new_count);

            // Only push if this (board, multiset) state is genuinely new
            if visited.insert((next_board, next_hash)) {
                stack.push((next_board, next_multiset, next_hash));
            }
        }
    }
    unique_solutions
}

#[allow(unused)]
fn canonicalize(
    unique_solutions: std::collections::HashSet<SolutionMultiset>,
    feasible: HashSet<Board>,
) -> Vec<[Board; 32]> {
    for s in &unique_solutions {
        for (s, c) in s {
            for _ in 0..*c {
                print!("{s} ");
            }
        }
        println!();
    }
    // canonicalize => sort multiset,
    // then always take first possible move on initial board.
    // Deduplicate by normalizing the boards and rehashing
    let unique_solutions: std::collections::HashSet<Solution> = unique_solutions
        .into_iter()
        .map(|b| Solution::from((b, &feasible)))
        .collect();
    log::info!(
        "unique solutions by move multiset: {}",
        unique_solutions.len()
    );
    for s in &unique_solutions {
        println!("{s}");
    }

    let unique_solutions: std::collections::HashSet<[Board; 32]> = unique_solutions
        .into_iter()
        .map(<[Board; 32]>::from)
        .map(|mut s| {
            s.iter_mut().for_each(|b| *b = b.normalize());
            s
        })
        .collect();
    let mut unique_solutions: Vec<_> = unique_solutions.into_iter().collect();
    unique_solutions.sort();

    unique_solutions
}

/// Precomputed random values for each (Step, occurrence_index) pair.
/// occurrence_index 0 means "going from 0 to 1 occurrences", etc.
#[derive(Default)]
struct ZobristTable {
    table: std::collections::HashMap<(Move, usize), u64>,
}

impl ZobristTable {
    fn delta(&mut self, step: &Move, new_count: usize) -> u64 {
        // XOR out the old count contribution, XOR in the new one
        let old = self.get(step, new_count - 1);
        let new = self.get(step, new_count);
        old ^ new
    }

    fn get(&mut self, step: &Move, count: usize) -> u64 {
        *self
            .table
            .entry((*step, count))
            .or_insert_with(rand::random)
    }
}

type MultisetHash = u64;

/// Upper bound on the moves available from one board.
///
/// The cross holds 38 collinear triples and each can be jumped from either end, so no
/// position can offer more than 76. Lets the successor buffer live on the stack instead of
/// being heap-allocated per board - the old code allocated a `Vec` for every one of the
/// 1_679_072 feasible boards.
const MAX_MOVES: usize = 76;

/// How to treat two different moves from the same board that reach the same normalized
/// successor - see [`all_unique_paths`] and [`all_unique_board_paths`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum PathKind {
    /// Count both. Paths are sequences of *moves*.
    MoveSequences,
    /// Count once. Paths are sequences of *normalized boards*.
    BoardSequences,
}

/// For every feasible board, the number of distinct **move sequences** taking it to the
/// solved board.
///
/// From the start board this is 40_861_647_040_079_968, the published solution count for the
/// central game, which is what pins this variant as correct.
///
/// Two different moves reaching the same normalized successor count as two paths here,
/// because they are two different sequences of moves. [`all_unique_board_paths`] is the
/// variant that collapses them.
pub fn all_unique_paths(
    feasible: impl IntoIterator<Item = Board>,
    threads: Option<NonZero<usize>>,
) -> HashMap<Board, u64> {
    count_paths(feasible, PathKind::MoveSequences, threads)
}

/// For every feasible board, the number of distinct sequences of **normalized boards**
/// taking it to the solved board.
///
/// Differs from [`all_unique_paths`] only where a board has two moves reaching the same
/// normalized successor - a board with a nontrivial stabilizer. From the start board this is
/// 4_750_671_971_732_176 against the move-sequence count's 40_861_647_040_079_968.
pub fn all_unique_board_paths(
    feasible: impl IntoIterator<Item = Board>,
    threads: Option<NonZero<usize>>,
) -> HashMap<Board, u64> {
    count_paths(feasible, PathKind::BoardSequences, threads)
}

/// Shared layered dynamic program: a board's count is the sum of its successors' counts, and
/// a move always removes exactly one peg, so layer `k` depends only on layer `k - 1`.
///
/// Three things this does differently from the obvious formulation, in decreasing order of
/// what they were worth:
///
/// - Successors come from `normalize_after_move`, not `possible_moves` + `normalize_all`. The
///   latter normalizes each successor from scratch, which is 8 symmetry transforms *per
///   successor*; the former takes the board's 8 symmetries once and XORs the move's mask into
///   each, which is the identity `g(b ^ m) = g(b) ^ g(m)` that `board.rs` already relies on.
///   Over 17.2M successors that is 137M symmetry transforms replaced by 17.2M XOR rounds.
/// - One `HashMap<Board, u32>` index instead of a `HashSet` plus a `HashMap`, so a successor
///   costs one hash lookup rather than two: finding it *is* finding where its count lives.
/// - The successor buffer is a stack array reused per board rather than a fresh `Vec`, which
///   removes one heap allocation per feasible board.
///
/// Each layer is then independent, so it is evaluated in parallel and written back in order.
fn count_paths(
    feasible: impl IntoIterator<Item = Board>,
    kind: PathKind,
    threads: Option<NonZero<usize>>,
) -> HashMap<Board, u64> {
    let mut index: HashMap<Board, u32> = HashMap::default();
    // `Board::SLOTS + 2` rather than 33: `count_pegs` reaches `SLOTS`, and the old fixed
    // `[_; 33]` would have panicked on a board with that many pegs. No feasible board has
    // (the start has one hole) but nothing here guarantees the input is the feasible set.
    let mut by_pegs: Vec<Vec<Board>> = vec![Vec::new(); Board::SLOTS + 2];
    for board in feasible {
        let next = index.len() as u32;
        index.insert(board, next);
        by_pegs[board.count_pegs()].push(board);
    }

    let mut counts = vec![0u64; index.len()];
    if let Some(&solved) = index.get(&Board::solved()) {
        counts[solved as usize] = 1;
    }

    // `None` means "as many as the machine has", matching `calculate_feasible_set`. Passing
    // the count down to `par::parallel` is what makes `--threads 1` actually sequential:
    // `par_map_chunks` short-circuits to one chunk on the calling thread at 1, whereas
    // `configure_thread_pool` cannot help - it leaves an already-built pool alone, so by this
    // point the pool's width is whatever the first caller asked for.
    let threads = threads.unwrap_or_else(par::num_threads).get();
    // skip(2): layer 0 is empty and layer 1 is the solved board, seeded above
    for layer in by_pegs.iter().skip(2) {
        if layer.is_empty() {
            continue;
        }
        // Read-only view of everything below this layer; the writes land afterwards, so no
        // two tasks touch the same count and none reads one this layer is producing.
        let (index_ref, counts_ref) = (&index, &counts);
        let layer_counts: Vec<u64> = par::parallel(layer, threads, move |chunk| {
            chunk
                .iter()
                .map(|board| {
                    let mut buffer = [Board::empty(); MAX_MOVES];
                    let successors = successors_of(*board, &mut buffer, kind);
                    successors
                        .iter()
                        .filter_map(|s| index_ref.get(s))
                        .map(|&i| counts_ref[i as usize])
                        .sum()
                })
                .collect()
        });
        for (board, count) in layer.iter().zip(layer_counts) {
            counts[index[board] as usize] = count;
        }
    }

    index
        .iter()
        .map(|(board, &i)| (*board, counts[i as usize]))
        .collect()
}

/// Fills `buffer` with `board`'s normalized successors and returns the filled prefix,
/// deduplicated when `kind` asks for it.
///
/// Deduplication sorts first, deliberately. A bare `dedup` removes only *adjacent* equals,
/// and `possible_moves` groups its output by direction, so two moves in different directions
/// reaching the same successor land far apart and survive it - 2_999 of them across the
/// feasible set, each double-counting a subtree. That is what the previous version of this
/// did, and it is why its answer matched neither of the two well-defined counts.
fn successors_of(board: Board, buffer: &mut [Board; MAX_MOVES], kind: PathKind) -> &[Board] {
    let syms = board.symmetries();
    let mut len = 0;
    for dir in Dir::enumerate() {
        for idx in board.mov_pattern_mask(dir) {
            debug_assert!(len < MAX_MOVES, "more than {MAX_MOVES} moves from one board");
            buffer[len] = Board::normalize_after_move(&syms, idx, dir);
            len += 1;
        }
    }
    let filled = &mut buffer[..len];
    if kind == PathKind::BoardSequences {
        filled.sort_unstable();
        // in-place dedup: `slice::partition_dedup` would do this but is still unstable
        let mut write = 0;
        for read in 0..len {
            if write == 0 || filled[write - 1] != filled[read] {
                filled[write] = filled[read];
                write += 1;
            }
        }
        len = write;
    }
    &buffer[..len]
}
