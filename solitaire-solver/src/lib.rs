mod board;
mod dag;
mod dir;
mod mov;
mod solution;

pub use board::{BOARD_SIZE, Board};
pub use dag::SolutionDag;
pub use dir::Dir;
use mov::Move;
pub use solution::Solution;

fn main() {
    let all = std::env::args().skip(1).any(|a| a == "-a");

    let mut board = Board::empty();

    board.set((2, 2));
    board.set((2, 3));
    board.set((3, 2));
    board.set((4, 2));

    for s in board.symmetries() {
        // println!("{s}");
    }

    if !all {
        let mut solution = Solution::default();
        let has_solution = solve(Default::default(), &mut solution);
        assert!(has_solution);
        // println!("{solution}");
    } else {
        let mut current = Solution::default();
        let mut dag = SolutionDag::new(Board::default());
        let has_solution = solve_all(Default::default(), &mut current, &mut dag);
        assert!(has_solution);
        // println!("checked {} constellations", dag.len());
        println!("{dag}");
    }
}

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

pub fn solve_all(board: Board, current: &mut Solution, solution_dag: &mut SolutionDag) -> bool {
    // found a known configuration
    match solution_dag.solutions(board) {
        Some(None) => return false,
        Some(_) => return true,
        _ => {}
    };

    // board is solved
    if board.is_solved() {
        // println!("solved!!");
        return true;
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
                    let modified = board.mov(mov).normalize();
                    if solve_all(modified, current, solution_dag) {
                        // println!("solution:\n{board}");
                        any_solution = true;
                        solution_dag.add_solution(board, modified);
                    }
                    current.pop();
                }
            }
        }
    }
    if !any_solution {
        solution_dag.no_solution(board);
    }

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
