use super::{Board, Dir, hash::CustomHashSet as HashSet};

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
        let mut copy = board.0;
        while copy != 0 {
            let idx = copy.trailing_zeros();
            copy &= !(1 << idx);
            let y = idx as i64 / Board::REPR;
            let x = idx as i64 % Board::REPR;
            for dir in [Dir::North, Dir::East, Dir::South, Dir::West] {
                if let Some(mov) = board.get_legal_move((y, x), dir) {
                    any_solution |=
                        solve_all(board.mov(mov).normalize(), already_checked, solvable);
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
