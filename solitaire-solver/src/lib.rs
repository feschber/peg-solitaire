mod board;
mod calc_first;
mod calc_naive;
mod calc_success;
mod dir;
mod hash;
mod mov;
mod solution;
mod sort;

use log::info;

pub use calc_first::calculate_first_solution;
pub use calc_naive::calculate_all_solutions_naive;
pub use calc_success::calculate_p_random_chance_success;
pub use solution::print_solution;
use voracious_radix_sort::RadixSort;

use std::{cmp::Ordering, num::NonZero, thread};

#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

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
    let uninit = result.spare_capacity_mut();
    let dsts = into_mut_slices(uninit, &lens);
    thread::scope(|s| {
        dsts.into_iter()
            .zip(slices)
            .map(|(dst, src)| {
                let dst: &mut [T] = unsafe { std::mem::transmute(dst) };
                s.spawn(|| dst.copy_from_slice(src.as_ref()))
            })
            .for_each(|_| {});
    });
    unsafe { result.set_len(total) };
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
            let mut mask = board.mov_pattern_mask(dir);
            while mask != Board::empty() {
                let idx = mask.0.trailing_zeros() as usize;
                mask &= Board(mask.0 - 1);
                constellations.push(board.toggle_mov_idx_unchecked(idx, dir));
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

#[allow(unused)]
fn normalize_dedup(mut constellations: Vec<Vec<Vec<Board>>>) -> Vec<Board> {
    let res = vec![];
    for dir in Dir::enumerate() {
        for idx in 0..(Board::REPR as usize * Board::REPR as usize) {
            let constellations = &mut constellations[dir as usize][idx];
            let (unchanged, normalized) = partition_normalize(constellations);
            assert!(unchanged.is_sorted());
            normalized.voracious_sort();
            let (unchanged, normalized) = (partition_dedup(unchanged), partition_dedup(normalized));
        }
    }
    res
}

fn partition_dedup(constellations: &mut [Board]) -> (&mut [Board], &mut [Board]) {
    let mut last = 0;
    for i in 0..constellations.len() {
        let c = constellations[i];
        let n = c.normalize();
        if n == c {
            constellations.swap(last, i);
            last = i + 1;
        }
    }
    constellations.split_at_mut(last)
}

fn partition_normalize(constellations: &mut [Board]) -> (&mut [Board], &mut [Board]) {
    let mut last = 0;
    for i in 0..constellations.len() {
        let c = constellations[i];
        let n = c.normalize();
        if n == c {
            constellations.swap(last, i);
            last = i + 1;
        }
    }
    constellations.split_at_mut(last)
}

#[cfg(target_arch = "wasm32")]
fn possible_moves_par(states: &[Board], _: usize) -> Vec<Board> {
    possible_moves(states)
}

#[cfg(not(target_arch = "wasm32"))]
fn possible_moves_par(states: &[Board], num_threads: usize) -> Vec<Board> {
    parallel(states, num_threads, possible_moves)
}

fn reverse_moves(states: &[Board]) -> Vec<Board> {
    let mut constellations = Vec::default();
    for dir in Dir::enumerate() {
        for board in states {
            let mut mask = board.rev_mov_pattern_mask(dir);
            while mask != Board::empty() {
                let idx = mask.0.trailing_zeros() as usize;
                mask &= Board(mask.0 - 1);
                constellations.push(board.toggle_mov_idx_unchecked(idx, dir));
            }
        }
    }
    normalize(&mut constellations);
    constellations
}

#[cfg(target_arch = "wasm32")]
fn reverse_moves_par(states: &[Board], _: usize) -> Vec<Board> {
    reverse_moves(states)
}

#[cfg(not(target_arch = "wasm32"))]
fn reverse_moves_par(states: &[Board], num_threads: usize) -> Vec<Board> {
    parallel(states, num_threads, reverse_moves)
}

pub fn calculate_all_solutions(threads: Option<NonZero<usize>>) -> Vec<Board> {
    #[cfg(not(target_arch = "wasm32"))]
    let start = Instant::now();
    #[cfg(not(target_arch = "wasm32"))]
    let mut time_sort = Duration::default();
    let threads = threads.unwrap_or(num_threads()).get();
    let mut visited = vec![vec![], vec![Board::solved()]];

    let mut total_constellations = 0;
    let mut total_moves = 0;
    info!(
        "{:>10} {:>10} {:>10}         {:>10}",
        "boards", "moves", "deduped", "intersection"
    );
    info!("-----------------------------------------------------");
    #[cfg(not(target_arch = "wasm32"))]
    let mut round = Instant::now();
    for i in 1..(Board::SLOTS - 1) / 2 {
        let num_constellations = visited[i].len();
        let mut constellations: Vec<Board> = reverse_moves_par(&visited[i], threads);
        #[cfg(not(target_arch = "wasm32"))]
        let rev_time = round.elapsed();
        let num_moves = constellations.len();
        #[cfg(not(target_arch = "wasm32"))]
        let start = Instant::now();
        constellations.fast_sort_unstable_mt(threads);
        #[cfg(not(target_arch = "wasm32"))]
        let sort = start.elapsed();
        #[cfg(not(target_arch = "wasm32"))]
        {
            time_sort += start.elapsed();
        }
        #[cfg(not(target_arch = "wasm32"))]
        let dd = Instant::now();
        let constellations = constellations.par_dedup(threads);
        #[cfg(not(target_arch = "wasm32"))]
        let dd = dd.elapsed();
        let deduped = constellations.len();
        visited.push(constellations);
        total_moves += num_moves;
        total_constellations += deduped;
        #[cfg(not(target_arch = "wasm32"))]
        let now = Instant::now();
        #[cfg(not(target_arch = "wasm32"))]
        let rt = now - round;
        #[cfg(not(target_arch = "wasm32"))]
        {
            round = now;
        }

        #[cfg(target_arch = "wasm32")]
        let rt = 0;
        #[cfg(target_arch = "wasm32")]
        let rev_time = 0;
        #[cfg(target_arch = "wasm32")]
        let sort = 0;
        #[cfg(target_arch = "wasm32")]
        let dd = 0;

        info!(
            "{num_constellations:>10} {num_moves:>10} {deduped:>10} ({:.1}%)                       {:>10?} (r: {:>10?}, s: {:>10?}, d: {:>10?})",
            deduped as f64 / num_moves as f64 * 100.,
            rt,
            rev_time,
            sort,
            dd,
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    let reverse_step = Instant::now();

    let mut inverted: Vec<_> = visited[(Board::SLOTS - 1) / 2]
        .iter()
        .map(|b| b.inverse())
        .collect();
    normalize(&mut inverted);
    inverted.fast_sort_unstable_mt(threads);
    visited.push(inverted);
    #[cfg(not(target_arch = "wasm32"))]
    let invert_step = Instant::now();

    #[cfg(not(target_arch = "wasm32"))]
    let mut round = Instant::now();
    for remaining in (2..=(Board::SLOTS - 1) / 2 + 1).rev() {
        let num_constellations = visited[remaining].len();
        let mut constellations = possible_moves_par(&visited[remaining], threads);
        #[cfg(not(target_arch = "wasm32"))]
        let t_moves = Instant::now();
        #[cfg(not(target_arch = "wasm32"))]
        let d_moves = t_moves.duration_since(round);
        let num_moves = constellations.len();
        total_moves += num_moves;
        #[cfg(not(target_arch = "wasm32"))]
        let start = Instant::now();
        constellations.fast_sort_unstable_mt(threads);
        #[cfg(not(target_arch = "wasm32"))]
        let t_sort = Instant::now();
        #[cfg(not(target_arch = "wasm32"))]
        let d_sort = t_sort.duration_since(t_moves);
        let deduped = constellations.len();
        #[cfg(not(target_arch = "wasm32"))]
        {
            time_sort += start.elapsed();
        }
        visited[remaining - 1] = intersect_sorted_vecs(&visited[remaining - 1], &constellations);
        #[cfg(not(target_arch = "wasm32"))]
        let t_intersect = Instant::now();
        #[cfg(not(target_arch = "wasm32"))]
        let d_intersect = t_intersect.duration_since(t_sort);
        let intersection = visited[remaining - 1].len();
        #[cfg(not(target_arch = "wasm32"))]
        let now = Instant::now();
        #[cfg(not(target_arch = "wasm32"))]
        let rt = now - round;
        #[cfg(not(target_arch = "wasm32"))]
        {
            round = now;
        }
        #[cfg(target_arch = "wasm32")]
        let rt = 0;
        #[cfg(target_arch = "wasm32")]
        let d_moves = 0;
        #[cfg(target_arch = "wasm32")]
        let d_sort = 0;
        #[cfg(target_arch = "wasm32")]
        let d_intersect = 0;
        info!(
            "{num_constellations:>10} {num_moves:>10} {deduped:>10} ({:.1}%) {intersection:>10} ({:.1}%)    {:>10?} (m: {:>10?}, s: {:>10?}, i: {:>10?})",
            deduped as f64 / num_moves as f64 * 100.,
            intersection as f64 / deduped as f64 * 100.,
            rt,
            d_moves,
            d_sort,
            d_intersect,
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    let forward_step = Instant::now();

    let solvable: Vec<Board> = visited
        .into_iter()
        .take((Board::SLOTS - 1) / 2 + 1)
        .flat_map(|s| s.into_iter().flat_map(|b| [b, b.inverse().normalize()]))
        .collect();
    #[cfg(not(target_arch = "wasm32"))]
    let collect_step = Instant::now();
    assert_eq!(solvable.len(), 1679072);
    info!("analyzed {total_moves} moves and {total_constellations} different constellations");
    #[cfg(not(target_arch = "wasm32"))]
    {
        info!("reverse step: {:?}", reverse_step.duration_since(start));
        info!(
            " invert step: {:?}",
            invert_step.duration_since(reverse_step)
        );
        info!(
            "forward step: {:?}",
            forward_step.duration_since(invert_step)
        );
        info!(
            "collect step: {:?}",
            collect_step.duration_since(forward_step)
        );
        info!("       total: {:?}", collect_step.duration_since(start));
        info!("     sorting: {time_sort:?}");
    }
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
    fn par_dedup(self, n_threads: usize) -> Self;
}

#[cfg(target_arch = "wasm32")]
impl<T: Copy + std::fmt::Debug + Send + Sync + PartialEq> ParDedup for Vec<T> {
    fn par_dedup(mut self, nthreads: usize) -> Self {
        self.dedup();
        self
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Copy + std::fmt::Debug + Send + Sync + PartialEq> ParDedup for Vec<T> {
    fn par_dedup(mut self, nthreads: usize) -> Self {
        if nthreads == 1 {
            self.dedup();
            return self;
        }
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
