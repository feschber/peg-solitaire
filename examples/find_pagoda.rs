//! Searches for a stronger pagoda function than the one in solitaire-solver/src/pagoda.rs.
//!
//! A pagoda function assigns a weight to each cell such that for every legal move
//! (peg jumps from `pos` over `mid` into `target`), `w(pos) + w(mid) >= w(target)` -
//! this makes total weight non-increasing along any forward move sequence, so
//! `pagoda(board) < pagoda(solved)` proves `board` can never reach `solved`.
//!
//! We restrict the search to weightings that respect the board's 8-fold symmetry
//! (one weight per symmetry orbit of the 33 cells, 7 orbits total) - this is both
//! principled (canonical/normalized boards are already symmetry classes) and makes
//! the search space small enough to brute force.
//!
//! Ground truth: run the real solver once to get the true feasible set, then
//! regenerate (unpruned) growth-phase rounds ourselves and label each candidate by
//! whether it's actually in the true feasible set. A valid weighting must NEVER
//! exclude a true positive; among weightings that don't, we want the one that
//! excludes the most false positives (true negatives).

use rayon::prelude::*;
use solitaire_solver::{Board, HashSet, Idx};
use std::collections::HashSet as StdHashSet;

// orbit id per cell index (idx = row*8+col, Board::REPR=8), derived from the
// dihedral-8 symmetry group acting on the 33 valid cells. -1 = unused/invalid cell.
const ORBIT: [i8; 64] = {
    let mut o = [-1i8; 64];
    // orbit 0: center
    o[27] = 0;
    let groups: [(&[usize], i8); 6] = [
        (&[3, 24, 30, 51], 1),
        (&[11, 25, 29, 43], 2), // existing pagoda's weighted cells (+ center)
        (&[18, 20, 34, 36], 3),
        (&[19, 26, 28, 35], 4),
        (&[2, 4, 16, 22, 32, 38, 50, 52], 5),
        (&[10, 12, 17, 21, 33, 37, 42, 44], 6),
    ];
    let mut g = 0;
    while g < groups.len() {
        let (cells, id) = groups[g];
        let mut i = 0;
        while i < cells.len() {
            o[cells[i]] = id;
            i += 1;
        }
        g += 1;
    }
    o
};

const NUM_ORBITS: usize = 7;

/// per-board orbit occupancy counts (how many pegs of each orbit are present).
fn orbit_counts(board: Board) -> [i32; NUM_ORBITS] {
    let mut counts = [0i32; NUM_ORBITS];
    for idx in board {
        let o = ORBIT[idx];
        if o >= 0 {
            counts[o as usize] += 1;
        }
    }
    counts
}

fn dot(counts: &[i32; NUM_ORBITS], weights: &[i64; NUM_ORBITS]) -> i64 {
    counts.iter().zip(weights).map(|(&c, &w)| c as i64 * w as i64).sum()
}

/// all geometric (pos, mid, target) move triples on the board, for every direction -
/// independent of any particular board's peg placement, this is just "which straight
/// lines of 3 consecutive valid cells exist".
fn move_triples() -> Vec<(usize, usize, usize)> {
    let mut triples = vec![];
    let idx = |y: Idx, x: Idx| -> usize { y as usize * Board::REPR as usize + x as usize };
    for y in 0..Board::SIZE {
        for x in 0..Board::SIZE {
            if !Board::inbounds((y, x)) {
                continue;
            }
            for (dy, dx) in [(0i8, 1i8), (0, -1), (1, 0), (-1, 0)] {
                let mid = (y + dy, x + dx);
                let tgt = (y + 2 * dy, x + 2 * dx);
                if Board::inbounds(mid) && Board::inbounds(tgt) {
                    triples.push((idx(y, x), idx(mid.0, mid.1), idx(tgt.0, tgt.1)));
                }
            }
        }
    }
    triples
}

fn is_valid_weighting(weights: &[i64; NUM_ORBITS], triples: &[(usize, usize, usize)]) -> bool {
    triples.iter().all(|&(pos, mid, tgt)| {
        let (op, om, ot) = (ORBIT[pos], ORBIT[mid], ORBIT[tgt]);
        weights[op as usize] + weights[om as usize] >= weights[ot as usize]
    })
}

fn growth_round(states: &[Board]) -> Vec<Board> {
    let mut next = Board::possible_reverse_moves(states);
    Board::normalize_all(&mut next);
    let set: StdHashSet<Board> = next.into_iter().collect();
    set.into_iter().collect()
}

fn main() {
    println!("computing true feasible set (ground truth)...");
    let feasible: HashSet<Board> = solitaire_solver::calculate_feasible_set(None).into_iter().collect();
    println!("feasible set size: {}", feasible.len());

    println!("regenerating growth-phase rounds (unpruned) for labeled data...");
    let mut frontier = vec![Board::solved()];
    let mut per_round: Vec<Vec<Board>> = vec![];
    for round in 0..15 {
        frontier = growth_round(&frontier);
        println!("  round {round}: {} boards", frontier.len());
        per_round.push(frontier.clone());
    }

    let all_candidates: Vec<Board> = per_round[10..].iter().flatten().copied().collect();
    let labeled: Vec<(Board, bool)> = all_candidates
        .into_iter()
        .map(|b| (b, feasible.contains(&b)))
        .collect();
    let num_pos = labeled.iter().filter(|(_, l)| *l).count();
    let num_neg = labeled.len() - num_pos;
    println!("labeled data: {num_pos} positives, {num_neg} negatives");

    // subsample negatives for search speed (positives are precious - keep them all,
    // since a single missed positive invalidates a weighting entirely).
    let mut rng_state = 0x2545F4914F6CDD1Du64;
    let mut next_rand = move || {
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 7;
        rng_state ^= rng_state << 17;
        rng_state
    };
    let sample_negs: Vec<Board> = labeled
        .iter()
        .filter(|(_, l)| !*l)
        .map(|(b, _)| *b)
        .filter(|_| next_rand() % 20 == 0) // ~5% sample
        .collect();
    let all_pos: Vec<Board> = labeled.iter().filter(|(_, l)| *l).map(|(b, _)| *b).collect();
    println!("search sample: {} positives (all), {} negatives (~5%)", all_pos.len(), sample_negs.len());

    let pos_counts: Vec<[i32; NUM_ORBITS]> = all_pos.iter().map(|&b| orbit_counts(b.inverse())).collect();
    let neg_counts: Vec<[i32; NUM_ORBITS]> = sample_negs.iter().map(|&b| orbit_counts(b.inverse())).collect();

    let triples = move_triples();
    println!("{} move constraint triples", triples.len());

    // sanity check: the existing pagoda.rs weighting (center=1, orbit2=1, rest=0)
    // must be valid and must never exclude a positive - if this fails, something in
    // our orbit/triple derivation is wrong.
    let existing = [1i64, 0, 1, 0, 0, 0, 0];
    assert!(is_valid_weighting(&existing, &triples), "existing pagoda weighting failed our own validity check - derivation bug");
    let existing_center = existing[0];
    let existing_false_prune = pos_counts.iter().filter(|c| dot(c, &existing) < existing_center).count();
    assert_eq!(existing_false_prune, 0, "existing pagoda weighting should never exclude a positive");
    let existing_pruned = neg_counts.iter().filter(|c| dot(c, &existing) < existing_center).count();
    println!(
        "sanity check ok: existing weighting prunes {existing_pruned}/{} sampled negatives ({:.2}%)",
        neg_counts.len(),
        100.0 * existing_pruned as f64 / neg_counts.len() as f64
    );

    println!("\nsearching weight space...");
    const RANGE: i64 = 3;
    let range: Vec<i64> = (-RANGE..=RANGE).collect();
    let mut candidates = vec![];
    for &a in &range {
        for &b in &range {
            for &c in &range {
                for &d in &range {
                    for &e in &range {
                        for &f in &range {
                            for &g in &range {
                                candidates.push([a, b, c, d, e, f, g]);
                            }
                        }
                    }
                }
            }
        }
    }
    println!("{} candidate weightings to check", candidates.len());

    let best = candidates
        .par_iter()
        .filter(|w| is_valid_weighting(w, &triples))
        .filter_map(|w| {
            let center = w[0];
            let false_prune = pos_counts.iter().any(|c| dot(c, w) < center);
            if false_prune {
                return None;
            }
            let pruned = neg_counts.iter().filter(|c| dot(c, w) < center).count();
            Some((pruned, *w))
        })
        .max_by_key(|(pruned, _)| *pruned);

    match best {
        Some((pruned, w)) => {
            println!(
                "\nBEST FOUND (on {}-sample): weights={w:?} prunes {pruned}/{} sampled negatives ({:.2}%)",
                RANGE,
                neg_counts.len(),
                100.0 * pruned as f64 / neg_counts.len() as f64
            );
            println!("(orbit reprs: 0=center 1=dist3-axis 2=dist2-axis(existing) 3=diag1 4=dist1-axis 5=dist(1,3) 6=dist(1,2))");

            // re-verify against the FULL dataset (not just the search sample) for a
            // trustworthy final number, and re-confirm validity + zero false-prune
            // independently of the search loop above.
            assert!(is_valid_weighting(&w, &triples));
            let center = w[0];
            let full_neg_counts: Vec<[i32; NUM_ORBITS]> =
                labeled.iter().filter(|(_, l)| !*l).map(|(b, _)| orbit_counts(b.inverse())).collect();
            let full_false_prune = pos_counts.iter().any(|c| dot(c, &w) < center);
            let full_pruned = full_neg_counts.iter().filter(|c| dot(c, &w) < center).count();
            println!(
                "FULL-DATA VERIFICATION: false_prune_any_positive={full_false_prune}, prunes {full_pruned}/{} negatives ({:.2}%)",
                full_neg_counts.len(),
                100.0 * full_pruned as f64 / full_neg_counts.len() as f64
            );

            println!("\nper-round pruning rate (existing vs found), applied to EVERY round (not just 10-14):");
            for (round, boards) in per_round.iter().enumerate() {
                let n = boards.len();
                let existing_pruned = boards
                    .iter()
                    .filter(|&&b| dot(&orbit_counts(b.inverse()), &existing) < existing_center)
                    .count();
                let found_pruned =
                    boards.iter().filter(|&&b| dot(&orbit_counts(b.inverse()), &w) < center).count();
                println!(
                    "  round {round:>2} ({n:>8} boards): existing {existing_pruned:>8} ({:>5.1}%)  found {found_pruned:>8} ({:>5.1}%)",
                    100.0 * existing_pruned as f64 / n as f64,
                    100.0 * found_pruned as f64 / n as f64,
                );
            }

            // the REAL test: apply the found weighting PROGRESSIVELY - prune each
            // round's survivors before using them to generate the next round - and
            // see the actual resulting sizes, not an extrapolation from
            // independently-measured per-round rates (which isn't valid once
            // earlier rounds have already been pruned).
            println!("\nprogressive pruning simulation (found weighting applied every round):");
            let mut pruned_frontier = vec![Board::solved()];
            let mut any_lost_positive = false;
            for round in 0..15 {
                pruned_frontier = growth_round(&pruned_frontier);
                let before = pruned_frontier.len();
                pruned_frontier.retain(|&b| dot(&orbit_counts(b.inverse()), &w) >= center);
                let after = pruned_frontier.len();
                // cross-check against ground truth: every board in this round that's
                // truly feasible must have survived the prune.
                let survivors: StdHashSet<Board> = pruned_frontier.iter().copied().collect();
                let lost_positive = per_round[round]
                    .iter()
                    .filter(|b| feasible.contains(b))
                    .any(|b| !survivors.contains(b));
                any_lost_positive |= lost_positive;
                println!(
                    "  round {round:>2}: unpruned would be {before:>8}, pruned to {after:>8} ({:>5.1}% reduction){}",
                    100.0 * (1.0 - after as f64 / before as f64),
                    if lost_positive { "  !!! LOST A TRUE POSITIVE !!!" } else { "" }
                );
            }
            println!(
                "\nfinal round size WITH progressive pruning: {} (vs {} unpruned - {:.1}% smaller)",
                pruned_frontier.len(),
                per_round[14].len(),
                100.0 * (1.0 - pruned_frontier.len() as f64 / per_round[14].len() as f64)
            );
            println!("any true positive ever lost during progressive pruning: {any_lost_positive}");
        }
        None => println!("\nno valid weighting found in range (unexpected)"),
    }
}
