mod board;
mod dir;
mod mov;
mod solution;

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

// 2^33
const BIT_SET_SIZE_BITS: u64 = 8_589_934_592;
const BIT_SET_CHUNKS: usize = (BIT_SET_SIZE_BITS / 64) as usize;
struct BitSet {
    bits: Box<[u64; BIT_SET_CHUNKS as usize]>,
}

impl BitSet {
    fn new() -> Self {
        let bits = Box::new([0u64; BIT_SET_CHUNKS as usize]);
        Self { bits }
    }
    fn test(&self, idx: u64) -> bool {
        let chunk = (idx / 64) as usize;
        let offset = (idx % 64) as usize;
        (self.bits[chunk] & (1 << offset)) != 0
    }
    fn set(&mut self, idx: u64) {
        let chunk = (idx / 64) as usize;
        let offset = (idx % 64) as usize;
        self.bits[chunk] |= 1 << offset;
    }
    fn count(&self) -> u64 {
        self.bits
            .iter()
            .copied()
            .map(|c| c.count_ones() as u64)
            .sum()
    }
}

pub fn calculate_all_solutions() -> Vec<Board> {
    let mut current = Solution::default();
    let mut solvable = BitSet::new();
    let mut already_checked = BitSet::new();
    solve_all(
        Board::default(),
        &mut current,
        &mut already_checked,
        &mut solvable,
    );
    let total = already_checked.count();
    let mut solvable_configurations = Vec::new();
    for i in 0..8_589_934_592 {
        if solvable.test(i) {
            solvable_configurations.push(Board::from_compressed_repr(i as u64));
        }
    }
    // let solvable: HashSet<_> = solvable
    //     .into_iter()
    //     .filter_map(|(board, solvable)| if solvable { Some(board) } else { None })
    //     .collect();
    let solvable = solvable_configurations.len();
    println!(
        "checked {total} constellations, {solvable} have a solution ({:.2}%)",
        (solvable as f64 / total as f64) * 100.
    );
    solvable_configurations
}

/// forward enumeration
fn solve_all(
    board: Board,
    current: &mut Solution,
    already_checked: &mut BitSet,
    solvable: &mut BitSet,
) -> bool {
    let compressed = board.to_compressed_repr();
    // board is solved
    if board.is_solved() {
        solvable.set(compressed);
        already_checked.set(compressed);
        return true;
    }

    // found a known configuration
    if already_checked.test(compressed) {
        return solvable.test(compressed);
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
                    any_solution |= solve_all(
                        board.mov(mov).normalize(),
                        current,
                        already_checked,
                        solvable,
                    );
                    current.pop();
                }
            }
        }
    }
    already_checked.set(compressed);
    if any_solution {
        solvable.set(compressed);
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
