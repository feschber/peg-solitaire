use crate::solution::SolutionMultiset;
use crate::{Board, Move};
use crate::{HashSet, Solution};
use std::collections::BTreeMap;

/// we define two solutions as "equal" when the
///  multiset of steps is equivalent between them

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
            if visited.insert((next_board.clone(), next_hash)) {
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

use std::collections::HashMap;

/// Precomputed random values for each (Step, occurrence_index) pair.
/// occurrence_index 0 means "going from 0 to 1 occurrences", etc.
#[derive(Default)]
struct ZobristTable {
    table: HashMap<(Move, usize), u64>,
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
            .entry((step.clone(), count))
            .or_insert_with(rand::random)
    }
}

type MultisetHash = u64;
