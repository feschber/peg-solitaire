use crate::{Board, Solution, hash::CustomHashSet as HashSet};

pub fn calculate_first_solution() -> Solution {
    fn solve(
        board: Board,
        solution: &mut Solution,
        visited: &mut HashSet<Board>,
        count: &mut u64,
    ) -> bool {
        *count += 1;
        if board.is_solved() {
            return true;
        }
        if !board.is_solvable() {
            return false;
        }
        if visited.contains(&board) {
            return false;
        }
        let mut legal_moves = board
            .get_legal_moves()
            .into_iter()
            .map(|m| (board.mov(m), m))
            .collect::<Vec<_>>();
        // for some reason sorting this way makes it orders of magnitude faster
        legal_moves.sort_unstable_by_key(|(b, _)| u64::MAX - b.0);
        legal_moves.dedup();
        for (b, m) in legal_moves {
            solution.push(m);
            if solve(b, solution, visited, count) {
                return true;
            }
            solution.pop();
        }
        visited.insert(board);
        false
    }
    let mut solution = Default::default();
    let mut visited = HashSet::default();
    let mut count = 0;
    solve(Board::default(), &mut solution, &mut visited, &mut count);
    println!("tried {count} constellations!");
    solution
}
