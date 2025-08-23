mod board;
mod dir;
mod mov;
mod solution;

use std::{collections::HashSet, hash::Hash, iter::repeat};

pub use board::Board;
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

pub fn calculate_all_solutions() -> Vec<Board> {
    let board = Board::default();
    let balls = board.count_balls();

    // let mut solvable = Vec::from_iter(repeat(HashSet::new()).take(balls as usize + 1));
    let mut visited = Vec::from_iter(repeat(HashSet::new()).take(balls as usize + 1));

    visited[board.count_balls() as usize] = HashSet::from_iter([board]);
    visited[Board::SLOTS - board.count_balls() as usize] = HashSet::from_iter([board.inverse()]);

    for remaining in ((Board::SLOTS - 1) / 2 + 2..=Board::SLOTS - 1).rev() {
        let [current, next, inverse] = visited
            .get_disjoint_mut([remaining, remaining - 1, Board::SLOTS - (remaining - 1)])
            .unwrap();
        for board in current.iter() {
            for mov in board.get_legal_moves() {
                let new = board.mov(mov).normalize();
                next.insert(new);
                inverse.insert(new.inverse());
            }
        }
    }

    for remaining in (1..=(Board::SLOTS - 1) / 2 + 1).rev() {
        let [current, next] = visited
            .get_disjoint_mut([remaining, remaining - 1])
            .unwrap();
        // retain reachable moves
        println!("current: {remaining}, next: {}", remaining - 1);
        let legal_moves = current
            .iter()
            .flat_map(|s| {
                s.get_legal_moves()
                    .into_iter()
                    .map(|m| s.mov(m).normalize())
            })
            .collect::<HashSet<_>>();
        next.retain(|b| legal_moves.contains(b));
    }

    for (i, v) in visited.iter().enumerate() {
        for b in v {
            assert_eq!(b.count_balls() as usize, i);
        }
        println!("reachable at {i}: {}", v.len());
    }

    vec![]
}

pub fn calculate_first_solution() -> Solution {
    let mut solution = Default::default();
    solve(Board::default(), &mut solution);
    solution
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
