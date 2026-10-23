//! Independent reference for `all_unique_paths`, to pin down what the right answer is.
//!
//! The library version deduplicates each board's successors with a bare `dedup()`, which
//! only removes *adjacent* equals. `possible_moves` groups its output by direction, so two
//! moves in different directions that normalize to the same successor land far apart and
//! survive - 2_999 of them across the feasible set, each double-counting a subtree.
//! `derive_graph` in the game crate documents the same hazard and sorts first.
//!
//! This computes it three ways so the difference is visible rather than argued.

use solitaire_solver::{Board, HashMap, HashSet, calculate_feasible_set};

fn paths(feasible: &[Board], dedup: fn(&mut Vec<Board>)) -> u64 {
    let set: HashSet<Board> = feasible.iter().copied().collect();
    let mut by_pegs: Vec<Vec<Board>> = vec![Vec::new(); Board::SLOTS + 2];
    for board in feasible {
        by_pegs[board.count_pegs()].push(*board);
    }
    let mut counts: HashMap<Board, u64> = HashMap::default();
    counts.insert(Board::solved(), 1);
    for pegs in 2..by_pegs.len() {
        for board in &by_pegs[pegs] {
            let mut next = Board::possible_moves(&[*board]);
            Board::normalize_all(&mut next);
            dedup(&mut next);
            let count = next
                .iter()
                .filter(|b| set.contains(b))
                .map(|b| counts[b])
                .sum();
            counts.insert(*board, count);
        }
    }
    counts[&Board::default()]
}

fn main() {
    env_logger::init();
    let feasible = calculate_feasible_set(None);
    println!("feasible: {}", feasible.len());
    println!(
        "  bare dedup() (what the library does): {}",
        paths(&feasible, |v| v.dedup())
    );
    println!(
        "  sort + dedup (distinct successors):   {}",
        paths(&feasible, |v| {
            v.sort_unstable();
            v.dedup();
        })
    );
    println!(
        "  no dedup at all (distinct moves):     {}",
        paths(&feasible, |_| {})
    );
}
