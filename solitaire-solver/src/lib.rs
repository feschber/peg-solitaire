mod board;
mod dir;
mod hash;
mod mov;
mod solution;

// use ahash::AHashSet as HashSet; // 1.194s
// use fnv::FnvHashSet as HashSet; // 1.024s
use hash::CustomHashSet as HashSet;
// use rustc_hash::FxHashSet as HashSet; // 0.866s
use std::{hash::Hash, num::NonZero, thread};

pub use board::Board;
pub use dir::Dir;
use mov::Move;
pub use solution::Solution;

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
        legal_moves.sort_by_key(|(b, _)| u64::MAX - b.0);
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

fn num_threads() -> NonZero<usize> {
    std::thread::available_parallelism().unwrap_or(NonZero::new(4).unwrap())
}

fn parallel<F, T, R>(states: &[T], num_threads: usize, f: F) -> HashSet<R>
where
    T: Send + Sync,
    F: Fn(&[T]) -> HashSet<R> + Send + Sync,
    R: Send + Eq + Hash + nohash_hasher::IsEnabled,
{
    #[cfg(target_family = "wasm")]
    {
        let _ = num_threads;
        return f(states);
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let mut chunks = states.chunks(states.len().div_ceil(num_threads));
        let result = thread::scope(|s| {
            let mut threads = Vec::with_capacity(num_threads - 1);
            let first_chunk = chunks.next().unwrap();
            for chunk in chunks {
                threads.push(s.spawn(|| f(chunk)));
            }
            // execute on current thread
            let mut result = f(first_chunk);
            for thread in threads {
                result.extend(thread.join().unwrap());
            }
            result
        });
        result
    }
}

fn possible_moves_par(states: &[Board], num_threads: usize) -> HashSet<Board> {
    parallel(states, num_threads, possible_moves)
}

fn reverse_moves_par(states: &[Board], num_threads: usize) -> HashSet<Board> {
    parallel(states, num_threads, reverse_moves)
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

pub fn calculate_all_solutions(threads: Option<NonZero<usize>>) -> Vec<Board> {
    let threads = threads.unwrap_or(num_threads()).into();
    let mut visited = vec![vec![], vec![Board::solved()]];

    for i in 1..(Board::SLOTS - 1) / 2 {
        let mut constellations: Vec<Board> = reverse_moves_par(&visited[i], threads)
            .into_iter()
            .collect();
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
        let legal_moves = possible_moves_par(&visited[remaining], threads);
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
