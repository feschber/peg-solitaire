//! Measures what a combinatorially-ranked key space would buy `keyset.rs`.
//!
//! Every board in a BFS round has the same peg count `k`, so the keys a round can
//! produce are not all `2^33` bit patterns but only the `C(33, k)` of them with
//! popcount `k` - up to 7.36x fewer. `DenseKeySet` currently indexes the raw
//! compressed key, so it spans 1 GiB regardless.
//!
//! Shrinking the bitmap only pays if the *touched* footprint shrinks with it, and
//! that is not obvious: `normalize` skews keys toward the low end of the range
//! (peak RSS of the 1 GiB mapping is 261 MiB, a quarter), and ranking is monotone,
//! so it may just preserve the skew rather than pack the keys densely. This prints
//! the distinct 4 KiB pages each scheme touches, which is what the page-fault cost
//! and the probe locality both follow.
//!
//! Analysis only - the ranking here is the plain O(k) form, not the two-table one
//! a hot path would want.

use solitaire_solver::Board;

const SLOTS: usize = 33;
const PAGE_BITS: u64 = 4096 * 8; // keys per 4 KiB page of bitmap

/// `binomial[n][k]` = C(n, k)
fn binomials() -> Vec<Vec<u64>> {
    let mut c = vec![vec![0u64; SLOTS + 2]; SLOTS + 2];
    for n in 0..=SLOTS + 1 {
        c[n][0] = 1;
        for k in 1..=n {
            // c[n-1][k] is legitimately 0 when k > n-1; the table is sized so the
            // index is always in range, so it must NOT be clamped
            c[n][k] = c[n - 1][k - 1] + c[n - 1][k];
        }
    }
    c
}

/// Colexicographic rank of `key` among the patterns with its own popcount:
/// `rank = sum over the i-th lowest set bit at position p of C(p, i + 1)`.
/// Bijective onto `0..C(33, popcount)`, and monotone in `key`.
fn colex_rank(mut key: u64, c: &[Vec<u64>]) -> u64 {
    let mut rank = 0;
    let mut i = 1;
    while key != 0 {
        let p = key.trailing_zeros() as usize;
        rank += c[p][i];
        key &= key - 1;
        i += 1;
    }
    rank
}

fn pages(keys: impl Iterator<Item = u64>) -> usize {
    let mut set: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for k in keys {
        set.insert(k / PAGE_BITS);
    }
    set.len()
}

fn main() {
    let c = binomials();

    // pin the table against known values before trusting anything built on it -
    // a silently wrong binomial makes every rank below wrong but still plausible
    assert_eq!(c[33][0], 1);
    assert_eq!(c[33][1], 33);
    assert_eq!(c[33][2], 528);
    assert_eq!(c[33][16], 1_166_803_110);
    assert_eq!(c[33][17], 1_166_803_110);
    assert_eq!(c[16][8], 12_870);
    assert_eq!(
        (0..=33).map(|k| c[33][k]).sum::<u64>(),
        1u64 << 33,
        "the row must sum to 2^33"
    );

    // rebuild the growth phase: reverse moves out from the solved board, normalized
    // and deduped, which is exactly what feeds the bitset rounds.
    let mut states = vec![Board::solved()];
    println!(
        "{:>5} {:>10} {:>16} {:>10} {:>10} {:>8} {:>7}",
        "pegs", "boards", "C(33,k)", "raw pages", "rank pages", "shrink", "touch%"
    );
    for round in 1..=17 {
        let mut next = Board::possible_reverse_moves(&states);
        Board::normalize_all(&mut next);
        next.sort_unstable_by_key(|b| b.to_compressed_repr());
        next.dedup();
        states = next;

        let pegs = states[0].count_pegs();
        assert!(
            states.iter().all(|b| b.count_pegs() == pegs),
            "round {round} mixes peg counts - the whole premise fails"
        );

        let keys: Vec<u64> = states.iter().map(|b| b.to_compressed_repr()).collect();
        let ranks: Vec<u64> = keys.iter().map(|&k| colex_rank(k, &c)).collect();
        assert!(
            ranks.iter().all(|&r| r < c[SLOTS][pegs]),
            "rank out of range for k={pegs}"
        );
        // the rank must stay injective, or the bitmap would merge distinct boards
        let mut sorted = ranks.clone();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "ranks collide at k={pegs}");

        let raw = pages(keys.iter().copied());
        let rank_pages = pages(ranks.iter().copied());
        println!(
            "{:>5} {:>10} {:>16} {:>10} {:>10} {:>7.2}x {:>6.1}%",
            pegs,
            states.len(),
            c[SLOTS][pegs],
            raw,
            rank_pages,
            raw as f64 / rank_pages as f64,
            rank_pages as f64 / (c[SLOTS][pegs] as f64 / PAGE_BITS as f64) * 100.,
        );
        if states.len() > 2_500_000 {
            break;
        }
    }
}
