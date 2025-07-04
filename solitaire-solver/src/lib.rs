mod board;
mod dir;
mod mov;
mod solution;

use std::collections::{HashMap, HashSet};

pub use board::{BOARD_SIZE, Board};
pub use dir::Dir;
use mov::Move;
pub use solution::Solution;

#[allow(unused)]
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

pub fn calculate_all_solutions() -> HashSet<u64> {
    let mut current = Solution::default();
    let mut solvable = HashMap::new();
    solve_all(Board::default(), &mut current, &mut solvable);
    let total_constellations = solvable.len();
    let solvable: HashSet<_> = solvable
        .into_iter()
        .filter_map(|(board, solvable)| if solvable { Some(board) } else { None })
        .collect();
    let solvable_configurations = solvable.len();
    println!(
        "checked {total_constellations} constellations, {solvable_configurations} have a solution ({:.2}%)",
        (solvable_configurations as f64 / total_constellations as f64) * 100.
    );
    solvable
}

fn solve_all(board: Board, current: &mut Solution, solvable: &mut HashMap<u64, bool>) -> bool {
    // board is solved
    if board.is_solved() {
        solvable.insert(board.0, true);
        return true;
    }

    // found a known configuration
    if let Some(&solvable) = solvable.get(&board.0) {
        return solvable;
    }

    let mut any_solution = false;
    for y in 0..BOARD_SIZE {
        for x in 0..BOARD_SIZE {
            if !board.occupied((y, x)) {
                continue;
            }
            for dir in [Dir::North, Dir::East, Dir::South, Dir::West] {
                if let Some(mov) = board.get_legal_move((y, x), dir) {
                    // println!("moving {:?} -> {dir:?}", (y, x));
                    current.push(mov);
                    any_solution |= solve_all(board.mov(mov).normalize(), current, solvable);
                    current.pop();
                }
            }
        }
    }
    solvable.insert(board.0, any_solution);
    any_solution
}

#[allow(unused)]
fn print_solution(solution: Solution) {
    let mut board = Board::default();
    println!("{board}");
    for mov in solution {
        board = board.mov(mov);
        println!("{mov}");
        println!("{board}");
    }
}
