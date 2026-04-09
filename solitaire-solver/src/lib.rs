mod board;
mod calc_first;
mod calc_naive;
mod calc_success;
mod dir;
mod dominators;
mod hash;
mod mov;
mod normalize_dedup;
mod pagoda;
mod par;
mod solution;
mod sort;
mod unique_solutions;

use log::info;

pub use calc_first::calculate_first_solution;
pub use calc_naive::calculate_all_solutions_naive;
pub use calc_success::calculate_p_random_chance_success;
pub use solution::print_solution;

use std::{cmp::Ordering, num::NonZero};

#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

pub use board::{Board, Idx};
pub use dir::Dir;
pub use hash::{CustomHashMap as HashMap, CustomHashSet as HashSet};
pub use mov::Move;
pub use solution::{Solution, SolutionMultiset};

pub use unique_solutions::{all_unique_paths, all_unique_solutions};

use crate::{par::ParDedup, sort::Sort};

fn possible_moves(states: &[Board]) -> Vec<Board> {
    let mut constellations = Board::possible_moves(states);
    normalize(&mut constellations);
    constellations
}

fn normalize(constellations: &mut [Board]) {
    for board in constellations {
        *board = board.normalize();
    }
}

#[cfg(target_arch = "wasm32")]
fn possible_moves_par(states: &[Board], _: usize) -> Vec<Board> {
    possible_moves(states)
}

#[cfg(not(target_arch = "wasm32"))]
fn possible_moves_par(states: &[Board], num_threads: usize) -> Vec<Board> {
    par::parallel(states, num_threads, possible_moves)
}

fn reverse_moves(states: &[Board]) -> Vec<Board> {
    let mut constellations = Board::possible_reverse_moves(states);
    normalize(&mut constellations);
    constellations
}

#[cfg(target_arch = "wasm32")]
fn reverse_moves_par(states: &[Board], _: usize) -> Vec<Board> {
    reverse_moves(states)
}

#[cfg(not(target_arch = "wasm32"))]
fn reverse_moves_par(states: &[Board], num_threads: usize) -> Vec<Board> {
    par::parallel(states, num_threads, reverse_moves)
}

pub fn calculate_all_solutions(threads: Option<NonZero<usize>>) -> Vec<Board> {
    #[cfg(not(target_arch = "wasm32"))]
    let start = Instant::now();
    #[cfg(not(target_arch = "wasm32"))]
    let mut time_sort = Duration::default();
    let threads = threads.unwrap_or(par::num_threads()).get();
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
