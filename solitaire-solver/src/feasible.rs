use std::time::Duration;

use std::{cmp::Ordering, num::NonZero};

use log::info;

use crate::{
    Board,
    par::{self, ParDedup},
    sort::Sort,
    timer::Timer,
};

fn possible_moves(states: &[Board]) -> Vec<Board> {
    let mut constellations = Board::possible_moves(states);
    Board::normalize_all(&mut constellations);
    constellations
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
    Board::normalize_all(&mut constellations);
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

pub fn calculate_feasible_set(threads: Option<NonZero<usize>>) -> Vec<Board> {
    let mut timer = Timer::new();
    let threads = threads.unwrap_or(par::num_threads()).get();
    let mut visited = vec![vec![], vec![Board::solved()]];
    let mut sort_time = Duration::ZERO;

    let mut total_constellations = 0;
    let mut total_moves = 0;
    info!(
        "{:>10} {:>10} {:>10}         {:>10}",
        "boards", "moves", "deduped", "intersection"
    );
    info!("-----------------------------------------------------");
    for i in 1..(Board::SLOTS - 1) / 2 {
        let mut timer = Timer::new();

        let num_constellations = visited[i].len();
        let mut constellations: Vec<Board> = reverse_moves_par(&visited[i], threads);

        timer.round("reverse".into());

        let num_moves = constellations.len();

        constellations.fast_sort_unstable_mt(threads);

        timer.round("sort".into());

        let constellations = constellations.par_dedup(threads);
        let deduped = constellations.len();
        visited.push(constellations);

        total_moves += num_moves;
        total_constellations += deduped;

        timer.round("dedup".into());

        info!(
            "{num_constellations:>10} {num_moves:>10} {deduped:>10} ({:>5.1}%)                        {:>12?} (r: {:>12?}, s: {:>12?}, d: {:>12?})",
            deduped as f64 / num_moves as f64 * 100.,
            timer.total(),
            timer.category("reverse".into()),
            timer.category("sort".into()),
            timer.category("dedup".into()),
        );
        sort_time += timer.category("sort".into());
    }

    timer.round("reverse step".into());

    let mut inverted: Vec<_> = visited[(Board::SLOTS - 1) / 2]
        .iter()
        .map(|b| b.inverse())
        .collect();
    Board::normalize_all(&mut inverted);
    inverted.fast_sort_unstable_mt(threads);
    visited.push(inverted);

    timer.round("inverse step".into());

    for remaining in (2..=(Board::SLOTS - 1) / 2 + 1).rev() {
        let mut timer = Timer::new();

        let num_constellations = visited[remaining].len();
        let mut constellations = possible_moves_par(&visited[remaining], threads);

        timer.round("moves".into());

        let num_moves = constellations.len();
        total_moves += num_moves;

        constellations.fast_sort_unstable_mt(threads);
        let constellations = constellations.par_dedup(threads);
        let deduped = constellations.len();

        timer.round("sort".into());

        visited[remaining - 1] = intersect_sorted_vecs(&visited[remaining - 1], &constellations);
        let intersection = visited[remaining - 1].len();

        timer.round("intersect".into());

        info!(
            "{num_constellations:>10} {num_moves:>10} {deduped:>10} ({:>5.1}%) {intersection:>10} ({:>5.1}%)    {:>12?} (m: {:>12?}, s: {:>12?}, i: {:>12?})",
            deduped as f64 / num_moves as f64 * 100.,
            intersection as f64 / deduped as f64 * 100.,
            timer.total(),
            timer.category("moves".into()),
            timer.category("sort".into()),
            timer.category("intersect".into()),
        );
        sort_time += timer.category("sort".into());
    }

    timer.round("forward".into());

    let solvable: Vec<Board> = visited
        .into_iter()
        .take((Board::SLOTS - 1) / 2 + 1)
        .flat_map(|s| s.into_iter().flat_map(|b| [b, b.inverse().normalize()]))
        .collect();

    timer.round("collect".into());

    assert_eq!(solvable.len(), 1679072);
    info!("analyzed {total_moves} moves and {total_constellations} different constellations");
    for (desc, dur) in timer.descriptions().zip(timer.durations()) {
        info!("{desc:>15}: {dur:>12?}");
    }
    info!("          total: {:>12?}", timer.total());
    info!("        sorting: {sort_time:?}");
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
