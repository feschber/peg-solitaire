mod board;
mod dir;
mod hash;
mod mov;
mod solution;

use rayon::slice::ParallelSliceMut;
// use ahash::AHashSet as HashSet; // 1.194s
// use fnv::FnvHashSet as HashSet; // 1.024s
use hash::CustomHashSet as HashSet;
// use rustc_hash::FxHashSet as HashSet; // 0.866s
use std::{cmp::Ordering, collections::HashMap, hash::Hash, num::NonZero, thread};

pub use board::Board;
pub use dir::Dir;
pub use mov::Move;
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

fn num_threads() -> NonZero<usize> {
    std::thread::available_parallelism().unwrap_or(NonZero::new(4).unwrap())
}

fn parallel<F, T, R>(states: &[T], num_threads: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    F: Fn(&[T]) -> Vec<R> + Send + Sync,
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
        thread::scope(|s| {
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
        })
    }
}

const PAGODA: [[f32; 7]; 7] = [
    [0.0, 0.0, -0.3, 0.4, 0.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0],
    [0.5, 0.0, 0.5, 0.4, 0.1, 0.3, -0.1],
    [0.0, 0.9, 0.7, 0.3, 0.9, 1.1, 0.4],
    [0.5, 0.6, 0.1, 0.5, 0.2, 0.6, 0.2],
    [0.0, 0.0, 0.8, 0.0, 0.8, 0.0, 0.0],
    [0.0, 0.0, 0.0, 0.5, -0.2, 0.0, 0.0],
];

fn pagoda(board: Board) -> f32 {
    let mut result = 0.;
    let mut copy = board.0;
    while copy != 0 {
        let idx = copy.trailing_zeros();
        let y = idx as i64 / Board::REPR;
        let x = idx as i64 % Board::REPR;
        copy &= !(1 << idx);
        result += PAGODA[y as usize][x as usize];
    }
    result
}

#[allow(unused)]
fn prune_pagoda(constellations: &mut Vec<Board>) {
    let len = constellations.len();
    constellations.retain(|b| {
        let pb = pagoda(*b);
        let pe = pagoda(Board::solved());
        pb >= pe
    });
    let new_len = constellations.len();
    let _diff = len - new_len;
    // println!("pruned {} configurations", diff);
}

fn possible_moves(states: &[Board]) -> Vec<Board> {
    let mut legal_moves = Vec::default();
    for dir in Dir::enumerate() {
        for board in states {
            let mut copy = *board & board.movable_positions(dir);
            while copy != Board::empty() {
                let idx = copy.0.trailing_zeros();
                let y = idx as i64 / Board::REPR;
                let x = idx as i64 % Board::REPR;
                copy &= Board(!(1 << idx));
                if let Some(mov) = board.get_legal_move((y, x), dir) {
                    legal_moves.push(board.mov(mov).normalize());
                }
            }
        }
    }
    // prune_pagoda(&mut legal_moves);
    legal_moves
}

fn reverse_moves(states: &[Board]) -> Vec<Board> {
    let mut constellations = Vec::default();
    for dir in Dir::enumerate() {
        for board in states {
            let mut copy = *board & board.movable_positions(dir);
            while copy != Board::empty() {
                let idx = copy.0.trailing_zeros();
                copy &= Board(!(1 << idx));
                let y = idx as i64 / Board::REPR;
                let x = idx as i64 % Board::REPR;
                if let Some(mov) = board.get_legal_inverse_move((y, x), dir) {
                    constellations.push(board.reverse_mov(mov).normalize());
                }
            }
        }
    }
    constellations
}

pub fn calculate_all_solutions(_threads: Option<NonZero<usize>>) -> Vec<Board> {
    let mut visited = vec![vec![], vec![Board::solved()]];

    for i in 1..(Board::SLOTS - 1) / 2 {
        let mut constellations: Vec<Board> = reverse_moves(&visited[i]);
        println!("constellations: {}", constellations.len());
        constellations.par_sort_unstable();
        constellations.dedup();
        visited.push(constellations);
    }

    visited.push(
        visited[(Board::SLOTS - 1) / 2]
            .iter()
            .map(|b| b.inverse().normalize())
            .collect(),
    );

    for remaining in (2..=(Board::SLOTS - 1) / 2 + 1).rev() {
        let mut legal_moves = possible_moves(&visited[remaining]);
        println!("{}", legal_moves.len());
        legal_moves.par_sort_unstable();
        legal_moves.dedup();
        visited[remaining - 1] = intersect_sorted_vecs(&visited[remaining - 1], &legal_moves);
    }

    let solvable: Vec<Board> = visited
        .into_iter()
        .take((Board::SLOTS - 1) / 2 + 1)
        .flat_map(|s| s.into_iter().flat_map(|b| [b, b.inverse().normalize()]))
        .collect();
    assert_eq!(solvable.len(), 1679072);
    solvable
}

/// calculate the chances of winning the game by chosing possible moves at random
pub fn calculate_p_random_chance_success(feasible: Vec<Board>) -> HashMap<Board, f64> {
    let feasible: HashSet<_> = feasible.into_iter().collect();
    let mut chances = HashMap::new();
    chances.insert(Board::solved(), 1.0);
    for i in 2..=(Board::SLOTS - 1) {
        let feasible_with_i_pegs = feasible
            .iter()
            .copied()
            .filter(|b| b.count_balls() == i as u64)
            .collect::<Vec<_>>();
        for constellation in feasible_with_i_pegs {
            let legal_moves = constellation.get_legal_moves();

            // we assume each legal move has equal chance of being taken (1 / n)
            // p_success = sum(moves, P(move) * P(success | move))
            // P(success | move) = 0.0 if infeasible, else lookup
            let p_move = 1.0 / legal_moves.len() as f64;

            let mut p_success = 0.0;

            for mov in legal_moves {
                let c_new = constellation.mov(mov).normalize();
                p_success += if feasible.contains(&c_new) {
                    p_move * *chances.get(&c_new).expect("already present")
                } else {
                    p_move * 0.0
                };
            }

            chances.insert(constellation, p_success);
        }
    }
    chances
}

fn intersect_sorted_vecs<R>(a: &[R], b: &[R]) -> Vec<R>
where
    R: Copy + Eq + Ord,
{
    let mut ia = 0;
    let mut ib = 0;
    let mut res = vec![];
    while ia < a.len() && ib < b.len() {
        match a[ia].cmp(&b[ib]) {
            Ordering::Equal => {
                res.push(a[ia]);
                ia += 1;
                ib += 1;
            }
            Ordering::Less => ia += 1,
            Ordering::Greater => ib += 1,
        }
    }
    res
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

pub fn print_solution(solution: Solution) {
    let mut board = Board::default();
    println!("{board}");
    for mov in solution {
        board = board.mov(mov);
        println!("{mov}");
        println!("{board}");
    }
}
