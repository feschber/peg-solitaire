mod board;
mod calc_first;
mod calc_naive;
mod calc_success;
mod dir;
mod hash;
mod mov;
mod solution;
mod sort;

pub use calc_first::calculate_first_solution;
pub use calc_naive::calculate_all_solutions_naive;
pub use calc_success::calculate_p_random_chance_success;
pub use solution::print_solution;

use std::{
    cmp::Ordering,
    hash::Hash,
    num::NonZero,
    thread,
    time::{Duration, Instant},
};

pub use board::Board;
pub use dir::Dir;
pub use hash::{CustomHashMap as HashMap, CustomHashSet as HashSet};
pub use mov::Move;
pub use solution::Solution;

use crate::sort::Sort;

fn num_threads() -> NonZero<usize> {
    std::thread::available_parallelism().unwrap_or(NonZero::new(4).unwrap())
}

fn parallel<F, T, R>(states: &[T], num_threads: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    F: Fn(&[T]) -> Vec<R> + Send + Sync,
    R: Copy + Send + Sync + Eq + Hash + nohash_hasher::IsEnabled,
{
    #[cfg(target_family = "wasm")]
    {
        let _ = num_threads;
        return f(states);
    }
    #[cfg(not(target_family = "wasm"))]
    {
        if num_threads == 1 || states.len() < 100 * num_threads {
            return f(states);
        } else {
            let mut chunks = states.chunks(states.len().div_ceil(num_threads));
            let results: Vec<Vec<R>> = thread::scope(|s| {
                let start = Instant::now();
                let mut threads = Vec::with_capacity(num_threads - 1);
                let first_chunk = chunks.next().unwrap();
                for chunk in chunks {
                    threads.push(s.spawn(|| f(chunk)));
                }
                // execute on current thread
                let mut results = vec![f(first_chunk)];
                results.extend(threads.into_iter().map(|t| t.join().unwrap()));
                results
            });
            let t_done = Instant::now();
            let mut total_len = results.iter().map(|r| r.len()).sum();
            let result = Vec::with_capacity(total_len);
            result.reserve(total_len);
            // SAFETY: we initialize below
            unsafe { result.set_len(result.len() + total_len) };

            let mut result_slices = vec![];
            let mut remaining = &mut result[..];
            for len in results.iter().map(|r| r.len()) {
                let (a, b) = remaining.split_at_mut(len);
                result_slices.push(a);
                remaining = b;
                total_len += len;
            }
            thread::scope(|s| {
                let mut collect_threads = vec![];
                for (slice, result) in result_slices.into_iter().zip(results) {
                    collect_threads.push(s.spawn(move || {
                        slice.copy_from_slice(&result);
                    }));
                }
                let t_collect = Instant::now();
                let t_processing = t_done.duration_since(start);
                let t_collect = t_collect.duration_since(t_done);
                println!("processing: {t_processing:?}, collecting: {t_collect:?}");
            });
            result
        }
    }
}

// somewhat effective
#[rustfmt::skip]
const PAGODA: [usize; 64] = [
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 1, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 1, 0, 1, 0, 1, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 1, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
];

fn pagoda(board: Board) -> usize {
    let mut result = 0;
    let mut copy = board.0;
    while copy != 0 {
        let idx = copy.trailing_zeros();
        copy &= !(1 << idx);
        result += PAGODA[idx as usize];
    }
    result
}

#[allow(unused)]
fn prune_pagoda_inverse(constellations: &mut Vec<Board>) {
    let len = constellations.len();
    constellations.retain(|&b| pagoda(b.inverse()) >= pagoda(Board::solved()));
    println!(
        "pruned {} configurations ({}%)",
        len - constellations.len(),
        (len - constellations.len()) as f32 / len as f32
    );
}

fn possible_moves(states: &[Board]) -> Vec<Board> {
    let mut legal_moves = Vec::default();
    for dir in Dir::enumerate() {
        for board in states {
            let mut copy = *board & board.movable_positions(dir);
            while copy != Board::empty() {
                let idx = copy.0.trailing_zeros();
                copy &= Board(!(1 << idx));
                if board.movable_at_no_bounds_check(idx as usize, dir) {
                    legal_moves.push(
                        board
                            .toggle_mov_idx_unchecked(idx as usize, dir)
                            .normalize(),
                    );
                }
            }
        }
    }
    legal_moves
}

fn possible_moves_par(states: &[Board], num_threads: usize) -> Vec<Board> {
    parallel(states, num_threads, possible_moves)
}

fn reverse_moves(states: &[Board]) -> Vec<Board> {
    let mut constellations = Vec::default();
    for dir in Dir::enumerate() {
        for board in states {
            let mut copy = *board & board.movable_positions(dir);
            while copy != Board::empty() {
                let idx = copy.0.trailing_zeros();
                copy &= Board(!(1 << idx));
                if board.reverse_movable_at_no_bounds_check(idx as usize, dir) {
                    constellations.push(
                        board
                            .toggle_mov_idx_unchecked(idx as usize, dir)
                            .normalize(),
                    );
                }
            }
        }
    }
    // prune_pagoda_inverse(&mut constellations);
    constellations
}

fn reverse_moves_par(states: &[Board], num_threads: usize) -> Vec<Board> {
    parallel(states, num_threads, reverse_moves)
}

pub fn calculate_all_solutions(threads: Option<NonZero<usize>>) -> Vec<Board> {
    let start = Instant::now();
    let mut time_sort = Duration::default();
    let threads = threads.unwrap_or(num_threads()).get();
    let mut visited = vec![vec![], vec![Board::solved()]];

    for i in 1..(Board::SLOTS - 1) / 2 {
        let mut constellations: Vec<Board> = reverse_moves_par(&visited[i], threads);
        println!("{}", constellations.len());
        let start = Instant::now();
        constellations.fast_sort_unstable_mt(threads);
        time_sort += start.elapsed();
        constellations.dedup();
        visited.push(constellations);
    }
    let reverse_step = Instant::now();

    visited.push(
        visited[(Board::SLOTS - 1) / 2]
            .iter()
            .map(|b| b.inverse())
            .collect(),
    );
    let invert_step = Instant::now();

    for remaining in (2..=(Board::SLOTS - 1) / 2 + 1).rev() {
        let mut legal_moves = possible_moves_par(&visited[remaining], threads);
        println!("{}", legal_moves.len());
        let start = Instant::now();
        legal_moves.fast_sort_unstable_mt(threads);
        time_sort += start.elapsed();
        visited[remaining - 1] = intersect_sorted_vecs(&visited[remaining - 1], &legal_moves);
    }
    let forward_step = Instant::now();

    let solvable: Vec<Board> = visited
        .into_iter()
        .take((Board::SLOTS - 1) / 2 + 1)
        .flat_map(|s| s.into_iter().flat_map(|b| [b, b.inverse().normalize()]))
        .collect();
    let collect_step = Instant::now();
    assert_eq!(solvable.len(), 1679072);
    println!("reverse step: {:?}", reverse_step.duration_since(start));
    println!(
        " invert step: {:?}",
        invert_step.duration_since(reverse_step)
    );
    println!(
        "forward step: {:?}",
        forward_step.duration_since(invert_step)
    );
    println!(
        "collect step: {:?}",
        collect_step.duration_since(forward_step)
    );
    println!("       total: {:?}", collect_step.duration_since(start));
    println!("     sorting: {time_sort:?}");
    solvable
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
