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
            for dir in Dir::enumerate() {
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
    let mut solvable = HashSet::new();
    let mut already_checked = HashSet::new();
    solve_all(Board::default(), &mut already_checked, &mut solvable);
    let total = already_checked.len();
    // let solvable: HashSet<_> = solvable
    //     .into_iter()
    //     .filter_map(|(board, solvable)| if solvable { Some(board) } else { None })
    //     .collect();
    let solvable_count = solvable.len();
    assert_eq!(solvable_count, 1679073);
    println!(
        "checked {total} constellations, {solvable_count} have a solution ({:.2}%)",
        (solvable_count as f64 / total as f64) * 100.
    );
    solvable.into_iter().collect()
}

pub fn calculate_first_solution() -> Solution {
    let mut solution = Default::default();
    solve(Board::default(), &mut solution);
    solution
}

/// forward enumeration
fn solve_all(
    board: Board,
    already_checked: &mut HashSet<Board>,
    solvable: &mut HashSet<Board>,
) -> bool {
    // board is solved
    if board.is_solved() {
        solvable.insert(board);
        already_checked.insert(board);
        return true;
    }

    // found a known configuration
    if already_checked.contains(&board) {
        return solvable.contains(&board);
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
                    any_solution |=
                        solve_all(board.mov(mov).normalize(), already_checked, solvable);
                }
            }
        }
    }
    already_checked.insert(board);
    if any_solution {
        solvable.insert(board);
    }
    any_solution
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
