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

/// maps n chunks of a slice `&[T]` into `R` in parallel using F
fn par_map_chunks<F, T, R>(t: impl AsRef<[T]>, nthreads: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    F: Fn(&[T]) -> R + Send + Sync,
    R: Default + Send + Sync,
{
    if nthreads == 1 || t.as_ref().len() < 100 * nthreads {
        vec![f(t.as_ref())]
    } else {
        let mut chunks = t.as_ref().chunks(t.as_ref().len().div_ceil(nthreads));
        thread::scope(|s| {
            let first_chunk = chunks.next().unwrap();
            let threads: Vec<_> = chunks.map(|c| s.spawn(|| f(c))).collect();

            // execute on current thread
            let mut results = vec![f(first_chunk)];
            results.extend(threads.into_iter().map(|t| t.join().unwrap()));
            results
        })
    }
}

/// maps n chunks of a slice `&[T]` into `R` in parallel using F
fn par_map_chunks_mut<F, T, R>(mut t: impl AsMut<[T]>, nthreads: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    F: Fn(&mut [T]) -> R + Send + Sync,
    R: Default + Send + Sync,
{
    if nthreads == 1 || t.as_mut().len() < 100 * nthreads {
        vec![f(t.as_mut())]
    } else {
        let chunk_size = t.as_mut().len().div_ceil(nthreads);
        let mut chunks = t.as_mut().chunks_mut(chunk_size);
        thread::scope(|s| {
            let first_chunk = chunks.next().unwrap();
            let threads: Vec<_> = chunks.map(|c| s.spawn(|| f(c))).collect();

            // execute on current thread
            let mut results = vec![f(first_chunk)];
            results.extend(threads.into_iter().map(|t| t.join().unwrap()));
            results
        })
    }
}

/// slices `v` into multiple mutable slices according to `lens` lengths
fn into_mut_slices<'a, T>(mut v: &'a mut [T], lens: &[usize]) -> Vec<&'a mut [T]> {
    let mut slices = vec![];
    assert_eq!(v.len(), lens.iter().sum());
    for len in lens {
        let (a, b) = v.split_at_mut(*len);
        slices.push(a);
        v = b;
    }
    slices
}

fn par_join<T: Copy + Send + Sync, VT: Send + Sync + AsRef<[T]>>(slices: &[VT]) -> Vec<T> {
    let lens = slices.iter().map(|r| r.as_ref().len()).collect::<Vec<_>>();
    let total = lens.iter().sum();
    let mut result = Vec::with_capacity(total);
    unsafe { result.set_len(total) };
    let dsts = into_mut_slices(&mut result, &lens);
    thread::scope(|s| {
        dsts.into_iter()
            .zip(slices)
            .map(|(dst, src)| s.spawn(|| dst.copy_from_slice(src.as_ref())))
            .for_each(|_| {});
    });
    result
}

fn parallel<F, T, R>(states: &[T], nthreads: usize, f: F) -> Vec<R>
where
    T: Send + Sync,
    F: Fn(&[T]) -> Vec<R> + Send + Sync,
    R: Copy + Default + Send + Sync,
{
    par_join(&par_map_chunks(states, nthreads, f))
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
    let mut constellations = Vec::default();
    for dir in Dir::enumerate() {
        for board in states {
            let mut mask = *board & board.movable_positions(dir);
            while mask != Board::empty() {
                let idx = mask.0.trailing_zeros() as usize;
                mask &= Board(mask.0 - 1);
                if board.movable_at_no_bounds_check(idx, dir) {
                    constellations.push(board.toggle_mov_idx_unchecked(idx, dir));
                }
            }
        }
    }
    normalize(&mut constellations);
    constellations
}

fn normalize(constellations: &mut [Board]) {
    for board in constellations {
        *board = board.normalize();
    }
}

fn possible_moves_par(states: &[Board], num_threads: usize) -> Vec<Board> {
    parallel(states, num_threads, possible_moves)
}

fn reverse_moves(states: &[Board]) -> Vec<Board> {
    let mut constellations = Vec::default();
    for dir in Dir::enumerate() {
        for board in states {
            let mut mask = *board & board.movable_positions(dir);
            while mask != Board::empty() {
                let idx = mask.0.trailing_zeros() as usize;
                mask &= Board(mask.0 - 1);
                if board.reverse_movable_at_no_bounds_check(idx, dir) {
                    constellations.push(board.toggle_mov_idx_unchecked(idx, dir));
                }
            }
        }
    }
    normalize(&mut constellations);
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

    let mut total_constellations = 0;
    let mut total_moves = 0;
    eprintln!(
        "{:>10} {:>10}         {:>10}",
        "original", "deduped", "intersection"
    );
    eprintln!("----------------------------------------");
    for i in 1..(Board::SLOTS - 1) / 2 {
        let mut constellations: Vec<Board> = reverse_moves_par(&visited[i], threads);
        let len = constellations.len();
        let start = Instant::now();
        constellations.fast_sort_unstable_mt(threads);
        time_sort += start.elapsed();
        let constellations = constellations.par_dedup(threads);
        let deduped = constellations.len();
        eprintln!(
            "{len:>10} {deduped:>10} ({:.1}%)",
            deduped as f64 / len as f64 * 100.
        );
        visited.push(constellations);
        total_moves += len;
        total_constellations += deduped;
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
        let mut constellations = possible_moves_par(&visited[remaining], threads);
        let len = constellations.len();
        let start = Instant::now();
        constellations.fast_sort_unstable_mt(threads);
        constellations.dedup();
        let deduped = constellations.len();
        time_sort += start.elapsed();
        visited[remaining - 1] = intersect_sorted_vecs(&visited[remaining - 1], &constellations);
        let intersection = visited[remaining - 1].len();
        eprintln!(
            "{len:>10} {deduped:>10} ({:.1}%) {intersection:>10} ({:.1}%)",
            deduped as f64 / len as f64 * 100.,
            intersection as f64 / deduped as f64 * 100.,
        );
    }
    let forward_step = Instant::now();

    let solvable: Vec<Board> = visited
        .into_iter()
        .take((Board::SLOTS - 1) / 2 + 1)
        .flat_map(|s| s.into_iter().flat_map(|b| [b, b.inverse().normalize()]))
        .collect();
    let collect_step = Instant::now();
    assert_eq!(solvable.len(), 1679072);
    eprintln!("analyzed {total_moves} moves and {total_constellations} different constellations");
    eprintln!("reverse step: {:?}", reverse_step.duration_since(start));
    eprintln!(
        " invert step: {:?}",
        invert_step.duration_since(reverse_step)
    );
    eprintln!(
        "forward step: {:?}",
        forward_step.duration_since(invert_step)
    );
    eprintln!(
        "collect step: {:?}",
        collect_step.duration_since(forward_step)
    );
    eprintln!("       total: {:?}", collect_step.duration_since(start));
    eprintln!("     sorting: {time_sort:?}");
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

trait ParDedup {
    fn par_dedup(&mut self, n_threads: usize) -> Self;
}

impl<T: Copy + std::fmt::Debug + Send + Sync + PartialEq> ParDedup for Vec<T> {
    fn par_dedup(&mut self, nthreads: usize) -> Self {
        let mut chunks: Vec<Vec<T>> = par_map_chunks_mut(self, nthreads, |c| {
            let mut v = Vec::from(c);
            v.dedup();
            v
        });
        for i in 0..chunks.len() - 1 {
            if chunks[i][chunks[i].len() - 1] == chunks[i + 1][0] {
                chunks[i].pop();
            }
        }
        par_join(&chunks)
    }
}
