mod board;
mod dir;
mod mov;
mod solution;

use std::collections::HashSet;

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

fn possible_moves<'a>(states: impl IntoIterator<Item = &'a Board>) -> HashSet<Board> {
    let mut legal_moves = HashSet::new();
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

fn reverse_moves<'a>(states: impl IntoIterator<Item = &'a Board>) -> HashSet<Board> {
    let mut constellations = HashSet::new();
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

    println!("possible constellations with {:>2} pegs: {:>7}", 1, 1);

    for i in 1..=(Board::SLOTS - 1) / 2 - 1 {
        let mut constellations: Vec<Board> = reverse_moves(&visited[i]).into_iter().collect();
        println!(
            "possible constellations with {:>2} pegs: {:>7}",
            i + 1,
            constellations.len()
        );
        constellations.sort_by_key(|b| b.0);
        visited.push(constellations);
    }

    visited.push(
        visited[(Board::SLOTS - 1) / 2]
            .iter()
            .map(|b| b.inverse().normalize())
            .collect(),
    );
    println!(
        "possible constellations with {:>2} pegs: {:>7}",
        visited.len() - 1,
        visited[visited.len() - 1].len()
    );

    for remaining in (2..=(Board::SLOTS - 1) / 2 + 1).rev() {
        let legal_moves = possible_moves(&visited[remaining]);
        visited[remaining - 1].retain(|b| legal_moves.contains(b));
        println!(
            "solvable constellations with {:>2} pegs: {:>7}",
            remaining - 1,
            visited[remaining - 1].len()
        );
    }

    let solvable: Vec<Board> = visited
        .into_iter()
        .take((Board::SLOTS - 1) / 2 + 1)
        .flat_map(|s| s.into_iter().flat_map(|b| [b, b.inverse().normalize()]))
        .collect();
    assert_eq!(solvable.len(), 1679072);
    solvable
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
