mod board;
mod dir;
mod mov;
mod solution;

use std::collections::HashSet;

pub use board::{BOARD_SIZE, Board};
pub use dir::Dir;
use mov::Move;
pub use solution::Solution;

fn solve(board: Board, solution: &mut Solution) -> bool {
    if board.is_solved() {
        return true;
    }
    if !board.is_solvable() {
        return false;
    }
    for y in 0..BOARD_SIZE {
        for x in 0..BOARD_SIZE {
            if !board.occupied((y, x)) {
                continue;
            }
            for dir in [Dir::North, Dir::East, Dir::South, Dir::West] {
                if let Some(mov) = board.get_legal_move((y, x), dir) {
                    solution.push(mov);
                    if solve(board.mov(mov), solution) {
                        return true;
                    }
                    solution.pop();
                }
            }
        }
    }
    false
}

pub fn calculate_all_solutions() -> Vec<Board> {
    let solvable = solve_all();
    println!("{} constellations have a solution", solvable.len() as f64);
    solvable
}

pub fn calculate_first_solution() -> Solution {
    let mut solution = Default::default();
    solve(Board::default(), &mut solution);
    solution
}

// forward enumeration
fn solve_all() -> Vec<Board> {
    let mut reachable_at: [Vec<Board>; 33] = std::array::from_fn(|_| Vec::new());
    reachable_at[32] = vec![Board::default()];

    let num_threads = 8;

    // forward dfs reachable constellations
    for remaining_balls in (1..=32).rev() {
        let len = (&reachable_at[remaining_balls]).len();
        let chunks: Vec<&[Board]> = (&reachable_at[remaining_balls])
            .chunks(len.div_ceil(num_threads).max(100))
            .collect();
        let reachable = std::thread::scope(|s| {
            let thread_start = std::time::Instant::now();
            let mut threads = Vec::with_capacity(num_threads);
            for window in chunks {
                let thread = s.spawn(move || {
                    let mut reachable = HashSet::new();
                    for board in window {
                        for y in 0..BOARD_SIZE {
                            for x in 0..BOARD_SIZE {
                                if board.occupied((y, x)) {
                                    for dir in Dir::enumerate() {
                                        if let Some(mov) = board.get_legal_move((y, x), dir) {
                                            reachable.insert(board.mov(mov).normalize());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    reachable
                });
                threads.push(thread);
            }
            let mut reachable = HashSet::new();
            let n_threads = threads.len();

            // join threads
            let mut sets = vec![];
            for thread in threads {
                sets.push(thread.join().unwrap());
            }
            let thread_end = std::time::Instant::now();

            // collect results
            for set in sets {
                reachable.extend(set);
            }
            let collecting_end = std::time::Instant::now();

            // read into vector
            let mut reachable: Vec<Board> = reachable.into_iter().collect();
            let vectorizing_end = std::time::Instant::now();

            // sort
            reachable.sort_by_key(|b| b.0);
            let sorting_end = std::time::Instant::now();

            println!(
                "{n_threads} threads took {:?}",
                thread_end.duration_since(thread_start)
            );
            println!(
                "collecting took {:?}",
                collecting_end.duration_since(thread_end)
            );
            println!(
                "vectorizing took {:?}",
                vectorizing_end.duration_since(collecting_end)
            );
            println!(
                "sorting took {:?}",
                sorting_end.duration_since(vectorizing_end)
            );
            println!();
            reachable
        });
        println!("reachable at {}: {}", remaining_balls - 1, reachable.len());
        reachable_at[remaining_balls - 1] = reachable;
    }

    vec![]
}

pub fn print_solution(solution: Solution) {
    let mut board = Board::default();
    println!("{board}");
    for mov in solution {
        board = board.mov(mov);
        println!("{mov}");
        println!("{board}");
    }
}
