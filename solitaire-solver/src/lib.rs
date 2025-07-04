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

#[cfg(feature = "precalculate")]
use std::{collections::HashSet, io::Read};
use std::{
    fs,
    io::{self, BufWriter, Write},
    path::Path,
};
#[cfg(feature = "precalculate")]
pub fn load_solutions() -> HashSet<Board> {
    // use std::time::Instant;

    // let time = Instant::now();
    println!("loading solutions");
    let mut solutions = HashSet::new();
    let compressed = include_bytes!("../../solutions.dat.br") as &[u8];
    let mut decompressor = brotli::Decompressor::new(compressed, 4096);
    let mut buf = [0u8; 4];
    decompressor.read_exact(&mut buf).expect("value");
    let count = u32::from_le_bytes(buf);
    for _ in 0..count {
        decompressor.read_exact(&mut buf).expect("value");
        let sol = u32::from_le_bytes(buf) as u64 | 0x1_0000_0000;
        solutions.insert(Board::from_compressed_repr(sol));
    }
    while let Ok(()) = decompressor.read_exact(&mut buf) {
        let sol = u32::from_le_bytes(buf) as u64;
        solutions.insert(Board::from_compressed_repr(sol));
    }
    // println!("loading done: took {:?}", time.elapsed());
    solutions
}

pub fn write_solutions<P>(solutions: Vec<Board>, p: P) -> Result<(), io::Error>
where
    P: AsRef<Path>,
{
    let solutions = solutions
        .into_iter()
        .map(|b| b.to_compressed_repr())
        .collect::<Vec<_>>();
    // solutions with the first bit set
    let sol_gt_u32 = solutions
        .iter()
        .filter(|&b| *b > u32::MAX as u64)
        .map(|&b| b as u32)
        .collect::<Vec<_>>();
    // solutions with the first bit not set
    let sol_lt_u32 = solutions
        .iter()
        .filter(|&b| *b <= u32::MAX as u64)
        .map(|&b| b as u32)
        .collect::<Vec<_>>();
    let count_gt_u32 = sol_gt_u32.len() as u32;
    let f = fs::File::create(p)?;
    let f = BufWriter::new(f);
    let mut compressor = brotli::CompressorWriter::new(f, 4096, 11, 22);
    let count_gt_u32 = count_gt_u32.to_le_bytes();
    compressor.write_all(&count_gt_u32)?;
    for b in sol_gt_u32 {
        let bytes = b.to_le_bytes();
        compressor.write_all(&bytes)?;
    }
    for b in sol_lt_u32 {
        let bytes = b.to_le_bytes();
        compressor.write_all(&bytes)?;
    }
    Ok(())
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
