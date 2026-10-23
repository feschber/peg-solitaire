//! How early in a game does a jump become unavoidable?
//!
//! Walks one winning line from the opening and, at each position, reports how many *future*
//! jumps every winning continuation from there must still make - i.e. the edge dominators of
//! the remaining game. That is the concrete form of "you played this, so you will have to
//! play that later".
//!
//! Both frames are shown, because they disagree and the difference is the point. `forced`
//! counts steps of the *normalized* graph, where symmetric alternatives collapse into one
//! edge, so it over-reports what the player is actually committed to. `jumps` counts literal
//! jumps in the game as played - the number a hint may show.

use std::time::Instant;

use solitaire_solver::dominators::{Verdict, classify_moves, forced_jumps, forced_moves};
use solitaire_solver::{Board, HashSet, all_unique_paths, calculate_feasible_set};

fn main() {
    env_logger::init();
    let feasible = calculate_feasible_set(None);
    let counts = all_unique_paths(feasible.clone(), None);
    let set: HashSet<Board> = feasible.iter().copied().collect();

    let mut board = Board::default();
    println!(" pegs   winning moves   forced   jumps   jump ms   lines remaining");
    while !board.is_solved() {
        let verdicts = classify_moves(board, &set, &counts);
        let winning: Vec<_> = verdicts
            .iter()
            .filter(|(_, v)| *v != Verdict::Losing)
            .collect();
        let forced = forced_moves(board, &set, &counts);
        let started = Instant::now();
        let jumps = forced_jumps(board, &counts);
        let elapsed = started.elapsed();
        let lines = counts.get(&board.normalize()).copied().unwrap_or(0);

        println!(
            "  {:2}          {:3}          {:3}     {:3}    {:6.1}   {lines}",
            board.count_pegs(),
            winning.len(),
            forced.len(),
            jumps.len(),
            elapsed.as_secs_f64() * 1e3,
        );

        // follow the first winning move; any would do
        let (mov, _) = winning
            .first()
            .expect("a winnable board has a winning move");
        board = board.mov(*mov);
    }

    println!("\nfirst position with a forced future jump, walked from the opening:");
    let mut board = Board::default();
    while !board.is_solved() {
        let jumps = forced_jumps(board, &counts);
        // a jump out of the current board is not news - the move colouring already shows it
        let later: Vec<_> = jumps.iter().filter(|f| f.board != board).collect();
        if !later.is_empty() {
            println!(
                "  at {} pegs, {} future jump(s) are already unavoidable",
                board.count_pegs(),
                later.len()
            );
            for f in later.iter().take(6) {
                println!(
                    "    in {:2} move(s) (at {:2} pegs): {}",
                    board.count_pegs() - f.board.count_pegs(),
                    f.board.count_pegs(),
                    f.mov
                );
            }
            break;
        }
        let verdicts = classify_moves(board, &set, &counts);
        let (mov, _) = verdicts
            .iter()
            .find(|(_, v)| *v != Verdict::Losing)
            .expect("winnable");
        board = board.mov(*mov);
    }
}
