mod board;
mod dir;
mod mov;
mod solution;

use rustc_hash::FxHashSet as HashSet;
use std::{num::NonZero, thread};

pub use board::Board;
pub use dir::Dir;
use mov::Move;
pub use solution::Solution;

pub fn calculate_first_solution() -> Solution {
    fn solve(board: Board, solution: &mut Solution) -> bool {
        if board.is_solved() {
            return true;
        }
        if !board.is_solvable() {
            return false;
        }
        for y in 0..Board::SIZE {
            for x in 0..Board::SIZE {
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
    let mut solution = Default::default();
    solve(Board::default(), &mut solution);
    solution
}

fn num_threads() -> usize {
    std::thread::available_parallelism()
        .unwrap_or(NonZero::new(4).unwrap())
        .into()
}

fn possible_moves_par(states: &[Board]) -> HashSet<Board> {
    let num_threads = num_threads();
    let chunks = states.chunks(states.len().div_ceil(num_threads));
    let result = thread::scope(|s| {
        let mut threads = Vec::with_capacity(num_threads);
        for chunk in chunks {
            threads.push(s.spawn(|| possible_moves(chunk)));
        }
        let mut result = HashSet::default();
        for thread in threads {
            if result.is_empty() {
                result = thread.join().unwrap();
            } else {
                result.extend(thread.join().unwrap());
            }
        }
        result
    });
    result
}

fn possible_moves(states: &[Board]) -> HashSet<Board> {
    let mut legal_moves = HashSet::default();
    for board in states {
        for y in 0..Board::SIZE {
            for x in 0..Board::SIZE {
                if board.occupied((y, x)) {
                    for dir in Dir::enumerate() {
                        if let Some(mov) = board.get_legal_move((y, x), dir) {
                            legal_moves.insert(board.mov(mov).normalize());
                        }
                    }
                }
            }
        }
    }
    legal_moves
}

fn reverse_moves_par(states: &[Board]) -> HashSet<Board> {
    let num_threads = num_threads();
    let chunks = states.chunks(states.len().div_ceil(num_threads));
    let result = thread::scope(|s| {
        let mut threads = Vec::with_capacity(num_threads);
        for chunk in chunks {
            threads.push(s.spawn(|| reverse_moves(chunk)));
        }
        let mut result = HashSet::default();
        for thread in threads {
            result.extend(thread.join().unwrap());
        }
        result
    });
    result
}

fn reverse_moves(states: &[Board]) -> HashSet<Board> {
    let mut constellations = HashSet::default();
    for board in states {
        for y in 0..Board::SIZE {
            for x in 0..Board::SIZE {
                if board.occupied((y, x)) {
                    for dir in Dir::enumerate() {
                        if let Some(mov) = board.get_legal_inverse_move((y, x), dir) {
                            constellations.insert(board.reverse_mov(mov).normalize());
                        }
                    }
                }
            }
        }
    }
    constellations
}

pub fn calculate_all_solutions() -> Vec<Board> {
    let mut visited = vec![vec![], vec![Board::solved()]];

    for i in 1..(Board::SLOTS - 1) / 2 {
        let mut constellations: Vec<Board> = reverse_moves_par(&visited[i]).into_iter().collect();
        constellations.sort_by_key(|b| b.0);
        visited.push(constellations);
    }

    visited.push(
        visited[(Board::SLOTS - 1) / 2]
            .iter()
            .map(|b| b.inverse().normalize())
            .collect(),
    );

    for remaining in (2..=(Board::SLOTS - 1) / 2 + 1).rev() {
        let legal_moves = possible_moves_par(&visited[remaining]);
        visited[remaining - 1].retain(|b| legal_moves.contains(b));
    }

    let solvable: Vec<Board> = visited
        .into_iter()
        .take((Board::SLOTS - 1) / 2 + 1)
        .flat_map(|s| s.into_iter().flat_map(|b| [b, b.inverse().normalize()]))
        .collect();
    assert_eq!(solvable.len(), 1679072);
    solvable
}

pub fn calculate_all_solutions_naive() -> Vec<Board> {
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
        for y in 0..Board::SIZE {
            for x in 0..Board::SIZE {
                if board.occupied((y, x)) {
                    for dir in [Dir::North, Dir::East, Dir::South, Dir::West] {
                        if let Some(mov) = board.get_legal_move((y, x), dir) {
                            any_solution |=
                                solve_all(board.mov(mov).normalize(), already_checked, solvable);
                        }
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
    let mut solvable = HashSet::default();
    let mut already_checked = HashSet::default();
    solve_all(Board::default(), &mut already_checked, &mut solvable);
    let total = already_checked.len();
    let solvable_count = solvable.len();
    assert_eq!(solvable_count, 1679072);
    println!(
        "checked {total} constellations, {solvable_count} have a solution ({:.2}%)",
        (solvable_count as f64 / total as f64) * 100.
    );
    solvable.into_iter().collect()
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
