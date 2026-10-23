//! Sample random winning lines and find the earliest position with a forced *future* jump.
use rand::prelude::*;
use solitaire_solver::dominators::forced_jumps;
use solitaire_solver::{Board, HashSet, all_unique_paths, calculate_feasible_set};

fn main() {
    let feasible = calculate_feasible_set(None);
    let counts = all_unique_paths(feasible.clone(), None);
    let _set: HashSet<Board> = feasible.iter().copied().collect();
    let mut rng = StdRng::seed_from_u64(0xC0FFEE);

    let mut earliest = vec![0usize; 34];
    let mut ahead_hist = vec![0usize; 40];
    let trials: usize = std::env::var("TRIALS")
        .map(|v| v.parse().unwrap())
        .unwrap_or(120);
    for _ in 0..trials {
        let mut board = Board::default();
        let mut best = 0usize;
        while !board.is_solved() {
            if board.count_pegs()
                <= std::env::var("FROM")
                    .map(|v| v.parse().unwrap())
                    .unwrap_or(26)
            {
                let jumps = forced_jumps(board, &counts);
                let later: Vec<_> = jumps.iter().filter(|f| f.board != board).collect();
                if !later.is_empty() {
                    best = board.count_pegs();
                    let min_ahead = later
                        .iter()
                        .map(|f| board.count_pegs() - f.board.count_pegs())
                        .min()
                        .unwrap();
                    let max_ahead = later
                        .iter()
                        .map(|f| board.count_pegs() - f.board.count_pegs())
                        .max()
                        .unwrap();
                    ahead_hist[min_ahead] += 1;
                    println!(
                        "  {:2} pegs: {} forced later, nearest {} move(s) ahead, farthest {}",
                        board.count_pegs(),
                        later.len(),
                        min_ahead,
                        max_ahead
                    );
                    break;
                }
            }
            let winning: Vec<_> = board
                .get_legal_moves()
                .into_iter()
                .filter(|mov| {
                    counts
                        .get(&board.mov(*mov).normalize())
                        .copied()
                        .unwrap_or(0)
                        > 0
                })
                .collect();
            board = board.mov(*winning.choose(&mut rng).unwrap());
        }
        earliest[best] += 1;
    }
    println!("\nearliest peg count with a forced future jump, over {trials} random winning lines:");
    for (pegs, n) in earliest.iter().enumerate().rev() {
        if *n > 0 {
            println!("  {pegs:2} pegs: {n}");
        }
    }
    println!("\nmoves until the nearest forced jump, at the position it first appears:");
    for (ahead, n) in ahead_hist.iter().enumerate() {
        if *n > 0 {
            println!("  {ahead:2} moves ahead: {n}");
        }
    }
}
