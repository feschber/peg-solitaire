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
    counts
        .iter()
        .zip(weights)
        .map(|(&c, &w)| c as i64 * w as i64)
        .sum()
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
    let feasible: HashSet<Board> = solitaire_solver::calculate_feasible_set(None)
        .into_iter()
        .collect();
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
    let all_pos: Vec<Board> = labeled
        .iter()
        .filter(|(_, l)| *l)
        .map(|(b, _)| *b)
        .collect();
    println!(
        "search sample: {} positives (all), {} negatives (~5%)",
        all_pos.len(),
        sample_negs.len()
    );

    let pos_counts: Vec<[i32; NUM_ORBITS]> =
        all_pos.iter().map(|&b| orbit_counts(b.inverse())).collect();
    let neg_counts: Vec<[i32; NUM_ORBITS]> = sample_negs
        .iter()
        .map(|&b| orbit_counts(b.inverse()))
        .collect();

    let triples = move_triples();
    println!("{} move constraint triples", triples.len());

    // sanity check: the existing pagoda.rs weighting (center=1, orbit2=1, rest=0)
    // must be valid and must never exclude a positive - if this fails, something in
    // our orbit/triple derivation is wrong.
    // The weighting actually in `pagoda.rs` today, read off its PAGODA table in
    // orbit form: center(27)=3, orbit1(3)=0, orbit2(11)=2, orbit3(18)=0,
    // orbit4(19)=2, orbit5(2)=-2, orbit6(10)=2. This used to be [1,0,1,0,0,0,0],
    // which is the *superseded* weighting - it prunes ~3.5% where production prunes
    // ~21%, so comparing a family against it would overstate the gain by 6x.
    let existing = [3i64, 0, 2, 0, 2, -2, 2];
    assert!(
        is_valid_weighting(&existing, &triples),
        "existing pagoda weighting failed our own validity check - derivation bug"
    );
    let existing_center = existing[0];
    let existing_false_prune = pos_counts
        .iter()
        .filter(|c| dot(c, &existing) < existing_center)
        .count();
    assert_eq!(
        existing_false_prune, 0,
        "existing pagoda weighting should never exclude a positive"
    );
    let existing_pruned = neg_counts
        .iter()
        .filter(|c| dot(c, &existing) < existing_center)
        .count();
    println!(
        "sanity check ok: existing weighting prunes {existing_pruned}/{} sampled negatives ({:.2}%)",
        neg_counts.len(),
        100.0 * existing_pruned as f64 / neg_counts.len() as f64
    );

    // ---------------------------------------------------------------------------
    // Phase 1: every weighting that is *usable*, not just the single best one.
    //
    // A weighting is usable if it is a valid pagoda (weight non-increasing along
    // every legal move) and excludes no true positive. The original search
    // collapsed this set to its best member; the whole point here is that its
    // members reject *different* boards, so their union is strictly stronger than
    // any one of them.
    // ---------------------------------------------------------------------------
    let range_max: i64 = std::env::var("PAGODA_RANGE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    println!("\nsearching weight space (range +-{range_max})...");
    let range: Vec<i64> = (-range_max..=range_max).collect();
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

    let mut usable: Vec<(usize, [i64; NUM_ORBITS])> = candidates
        .par_iter()
        .filter(|w| is_valid_weighting(w, &triples))
        .filter_map(|w| {
            let center = w[0];
            if pos_counts.iter().any(|c| dot(c, w) < center) {
                return None;
            }
            let pruned = neg_counts.iter().filter(|c| dot(c, w) < center).count();
            (pruned > 0).then_some((pruned, *w))
        })
        .collect();
    usable.sort_unstable_by_key(|(pruned, _)| std::cmp::Reverse(*pruned));
    println!(
        "{} usable weightings (valid + exclude no positive + prune something)",
        usable.len()
    );
    match usable.first() {
        Some((pruned, w)) => println!(
            "best single: weights={w:?} prunes {pruned}/{} sampled negatives ({:.2}%)",
            neg_counts.len(),
            100.0 * *pruned as f64 / neg_counts.len() as f64
        ),
        None => {
            println!("no usable weighting found - nothing to do");
            return;
        }
    }
    println!(
        "(orbit reprs: 0=center 1=dist3-axis 2=dist2-axis(existing) 3=diag1 4=dist1-axis 5=dist(1,3) 6=dist(1,2))"
    );

    // ---------------------------------------------------------------------------
    // Phase 2: greedy set cover over the negatives.
    //
    // Restricted to the strongest POOL weightings by individual coverage, purely to
    // bound the memory of the rejection bitsets - a weighting that rejects very few
    // negatives cannot contribute much marginal coverage either.
    // ---------------------------------------------------------------------------
    const POOL: usize = 512;
    const MAX_FAMILY: usize = 8;
    let pool: Vec<[i64; NUM_ORBITS]> = usable.iter().take(POOL).map(|(_, w)| *w).collect();
    let words = neg_counts.len().div_ceil(64);
    let masks: Vec<Vec<u64>> = pool
        .par_iter()
        .map(|w| {
            let center = w[0];
            let mut bits = vec![0u64; words];
            for (i, c) in neg_counts.iter().enumerate() {
                if dot(c, w) < center {
                    bits[i >> 6] |= 1 << (i & 63);
                }
            }
            bits
        })
        .collect();

    println!("\ngreedy family selection (pool of {}):", pool.len());
    let mut covered = vec![0u64; words];
    let mut family: Vec<[i64; NUM_ORBITS]> = vec![];
    let total_negs = neg_counts.len();
    while family.len() < MAX_FAMILY {
        let best = masks
            .par_iter()
            .enumerate()
            .map(|(i, m)| {
                let gain: usize = m
                    .iter()
                    .zip(&covered)
                    .map(|(a, b)| (a & !b).count_ones() as usize)
                    .sum();
                (gain, i)
            })
            .max();
        let Some((gain, i)) = best else { break };
        // stop once a further member buys less than half a percent of the negatives:
        // every member costs a dot product on every extracted board at runtime
        if gain * 200 < total_negs {
            println!("  stopping: best marginal gain {gain} is under 0.5% of negatives");
            break;
        }
        for (c, m) in covered.iter_mut().zip(&masks[i]) {
            *c |= m;
        }
        family.push(pool[i]);
        let now: usize = covered.iter().map(|w| w.count_ones() as usize).sum();
        println!(
            "  +{:?}  marginal {gain:>7} -> cumulative {now:>7}/{total_negs} ({:.2}%)",
            pool[i],
            100.0 * now as f64 / total_negs as f64
        );
    }

    // ---------------------------------------------------------------------------
    // Phase 3: re-verify the chosen family against the FULL data, independently of
    // the sampled search above. A single missed positive invalidates the family, so
    // this is checked per member as well as for the union.
    // ---------------------------------------------------------------------------
    let survives_family = |b: Board, fam: &[[i64; NUM_ORBITS]]| -> bool {
        let c = orbit_counts(b.inverse());
        fam.iter().all(|w| dot(&c, w) >= w[0])
    };
    let full_negs: Vec<Board> = labeled
        .iter()
        .filter(|(_, l)| !*l)
        .map(|(b, _)| *b)
        .collect();
    for (i, w) in family.iter().enumerate() {
        assert!(
            is_valid_weighting(w, &triples),
            "family member {i} is not a valid pagoda"
        );
        assert!(
            !all_pos
                .iter()
                .any(|&b| dot(&orbit_counts(b.inverse()), w) < w[0]),
            "family member {i} excludes a true positive"
        );
    }
    let existing_full = full_negs
        .iter()
        .filter(|&&b| dot(&orbit_counts(b.inverse()), &existing) < existing_center)
        .count();
    let family_full = full_negs
        .iter()
        .filter(|&&b| !survives_family(b, &family))
        .count();
    println!(
        "\nFULL-DATA: existing prunes {existing_full}/{} ({:.2}%), family of {} prunes {family_full} ({:.2}%)",
        full_negs.len(),
        100.0 * existing_full as f64 / full_negs.len() as f64,
        family.len(),
        100.0 * family_full as f64 / full_negs.len() as f64
    );

    // ---------------------------------------------------------------------------
    // Phase 4: the number that actually decides this - progressive pruning. Prune
    // each round before it generates the next, so the compounding is measured
    // rather than extrapolated from independent per-round rates.
    // ---------------------------------------------------------------------------
    println!("\nprogressive pruning (existing single vs family), applied every round:");
    let simulate = |fam: &[[i64; NUM_ORBITS]]| -> (Vec<usize>, bool) {
        let mut frontier = vec![Board::solved()];
        let mut sizes = vec![];
        let mut lost = false;
        for round in 0..15 {
            frontier = growth_round(&frontier);
            frontier.retain(|&b| survives_family(b, fam));
            let survivors: StdHashSet<Board> = frontier.iter().copied().collect();
            lost |= per_round[round]
                .iter()
                .filter(|b| feasible.contains(b))
                .any(|b| !survivors.contains(b));
            sizes.push(frontier.len());
        }
        (sizes, lost)
    };
    let (base_sizes, base_lost) = simulate(&[existing]);
    // ties this simulation to reality: the real solver's `visited[16]` is 2046865,
    // so if the orbit form of the production weighting is right, round 14 of the
    // baseline simulation must land on exactly that
    assert_eq!(
        base_sizes[14], 2_046_865,
        "baseline simulation does not reproduce the real solver's final growth round - \
         the orbit form of the production weighting is wrong"
    );
    let (fam_sizes, fam_lost) = simulate(&family);
    assert!(
        !base_lost,
        "the existing weighting lost a positive - harness bug"
    );
    assert!(
        !fam_lost,
        "THE FAMILY LOST A TRUE POSITIVE - it is not sound"
    );

    println!(
        "  {:>5} {:>10} {:>10} {:>10}",
        "round", "today", "family", "reduction"
    );
    let (mut tot_a, mut tot_b) = (0usize, 0usize);
    for (round, (a, b)) in base_sizes.iter().zip(&fam_sizes).enumerate() {
        tot_a += a;
        tot_b += b;
        println!(
            "  {round:>5} {a:>10} {b:>10} {:>9.1}%",
            100.0 * (1.0 - *b as f64 / *a as f64)
        );
    }
    println!(
        "  {:>5} {tot_a:>10} {tot_b:>10} {:>9.1}%",
        "total",
        100.0 * (1.0 - tot_b as f64 / tot_a as f64)
    );
    println!("\nboards carried by the growth phase: {tot_a} -> {tot_b}");
    println!("family: {family:?}");
    println!("no true positive lost: {}", !fam_lost);
}
