//! Isolates the payoff of storing the shrink phase's probe side as `u32` ranks
//! instead of `Board`s, which is the largest single component of narrowing
//! `visited` to ranks - and the one that has to carry the change, since ranks are
//! layer-relative and every *generation* source would then need decoding back.
//!
//! `feasible.rs`'s `intersect_chunk` walks `visited[remaining - 1]`, and for each
//! board computes `to_compressed_repr` (a `pext`), ranks it (two table reads), then
//! prefetches and probes the map - carrying the bit position in a ring buffer so
//! the rank is computed once rather than twice. If that vector instead held the
//! ranks themselves, all of that disappears: the stored `u32` *is* the bit
//! position, so the ring buffer goes too, and the scan reads 4 bytes per board
//! instead of 8.
//!
//! Both variants below are otherwise the same loop over the same boards against the
//! same map, and both push their survivors, so the output side is represented too.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use rayon::prelude::*;
use solitaire_solver::Board;

const SLOTS: usize = 33;
const LOW_BITS: u32 = 16;
const LOW_MASK: u64 = (1 << LOW_BITS) - 1;
/// matches `par::parallel`'s per-chunk parallelism in the real intersect
const CHUNK: usize = 16_384;
const PREFETCH_DISTANCE: usize = 16;

fn binomials() -> Vec<Vec<u64>> {
    let mut c = vec![vec![0u64; SLOTS + 2]; SLOTS + 2];
    for n in 0..=SLOTS + 1 {
        c[n][0] = 1;
        for k in 1..=n {
            c[n][k] = c[n - 1][k - 1] + c[n - 1][k];
        }
    }
    c
}

fn low_rank_table() -> Vec<u16> {
    let mut t = vec![0u16; 1 << LOW_BITS];
    let mut counters = [0u16; LOW_BITS as usize + 1];
    for l in 0..(1u32 << LOW_BITS) {
        let pc = l.count_ones() as usize;
        t[l as usize] = counters[pc];
        counters[pc] += 1;
    }
    t
}

fn high_cum_table(k: usize, c: &[Vec<u64>]) -> Vec<u32> {
    let highs = 1usize << (SLOTS as u32 - LOW_BITS);
    let mut t = vec![0u32; highs];
    let mut acc = 0u64;
    for (h, slot) in t.iter_mut().enumerate() {
        *slot = acc as u32;
        let hp = (h as u64).count_ones() as usize;
        if hp <= k && (k - hp) <= LOW_BITS as usize {
            acc += c[LOW_BITS as usize][k - hp];
        }
    }
    assert_eq!(acc, c[SLOTS][k], "high_cum must total C(33,k)");
    t
}

#[inline]
fn rank(key: u64, high_cum: &[u32], low_rank: &[u16]) -> u64 {
    high_cum[(key >> LOW_BITS) as usize] as u64 + low_rank[(key & LOW_MASK) as usize] as u64
}

fn zeroed(words: usize) -> Vec<AtomicU64> {
    let z: Vec<u64> = vec![0; words];
    unsafe { std::mem::transmute::<Vec<u64>, Vec<AtomicU64>>(z) }
}

#[inline]
fn prefetch(words: &[AtomicU64], idx: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_mm_prefetch::<{ core::arch::x86_64::_MM_HINT_T0 }>(
            words.as_ptr().add(idx).cast::<i8>(),
        );
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = (words, idx);
}

#[inline]
fn test_at(words: &[AtomicU64], bit: u64) -> bool {
    (words[(bit >> 6) as usize].load(Ordering::Relaxed) >> (bit & 63)) & 1 != 0
}

/// `feasible.rs`'s `intersect_chunk`, verbatim in shape: rank each board, carry the
/// bit in a ring so it is computed once, prefetch `PREFETCH_DISTANCE` ahead.
fn intersect_boards(words: &[AtomicU64], chunk: &[Board], hc: &[u32], lr: &[u16]) -> Vec<Board> {
    let mut out = Vec::with_capacity(chunk.len());
    let mut ring = [0u64; PREFETCH_DISTANCE];
    for (slot, board) in chunk.iter().take(PREFETCH_DISTANCE).enumerate() {
        let bit = rank(board.to_compressed_repr(), hc, lr);
        prefetch(words, (bit >> 6) as usize);
        ring[slot] = bit;
    }
    let split = chunk.len().saturating_sub(PREFETCH_DISTANCE);
    let ahead_of = chunk.get(PREFETCH_DISTANCE..).unwrap_or(&[]);
    for (i, (board, ahead)) in chunk[..split].iter().zip(ahead_of).enumerate() {
        let slot = i & (PREFETCH_DISTANCE - 1);
        let bit = ring[slot];
        let ahead_bit = rank(ahead.to_compressed_repr(), hc, lr);
        prefetch(words, (ahead_bit >> 6) as usize);
        ring[slot] = ahead_bit;
        if test_at(words, bit) {
            out.push(*board);
        }
    }
    for (i, board) in chunk[split..].iter().enumerate() {
        if test_at(words, ring[(split + i) & (PREFETCH_DISTANCE - 1)]) {
            out.push(*board);
        }
    }
    out
}

/// the same filter if the vector held ranks: the stored value already *is* the bit
/// position, so there is no key, no table read, and no ring to carry it in.
fn intersect_ranks(words: &[AtomicU64], chunk: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(chunk.len());
    for &r in chunk.iter().take(PREFETCH_DISTANCE) {
        prefetch(words, (r >> 6) as usize);
    }
    let split = chunk.len().saturating_sub(PREFETCH_DISTANCE);
    let ahead_of = chunk.get(PREFETCH_DISTANCE..).unwrap_or(&[]);
    for (r, ahead) in chunk[..split].iter().zip(ahead_of) {
        prefetch(words, (ahead >> 6) as usize);
        if test_at(words, *r as u64) {
            out.push(*r);
        }
    }
    for r in &chunk[split..] {
        if test_at(words, *r as u64) {
            out.push(*r);
        }
    }
    out
}

fn main() {
    let c = binomials();
    let reps: usize = std::env::var("REPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9);

    // rebuild the growth phase far enough to get a realistic shrink round: `states`
    // generates the moves that fill the map, `probe` is the growth-phase side the
    // real round intersects against.
    eprintln!("building rounds...");
    let mut states = vec![Board::solved()];
    let probe = loop {
        let mut next = Board::possible_reverse_moves(&states);
        Board::normalize_all(&mut next);
        next.sort_unstable_by_key(|b| b.to_compressed_repr());
        next.dedup();
        let prev = std::mem::replace(&mut states, next);
        if states.len() > 2_400_000 {
            break prev;
        }
    };
    let k = probe[0].count_pegs();
    let mut moves = Board::possible_moves(&states);
    Board::normalize_all(&mut moves);
    assert_eq!(
        moves[0].count_pegs(),
        k,
        "probe side must share the map's layer"
    );

    let hc = high_cum_table(k, &c);
    let lr = low_rank_table();

    // Two maps, because the survival rate changes what the comparison is measuring.
    // Filling from every move makes almost the whole probe side survive, which is
    // not what the real round does - it keeps 12-20% - and a high survival rate
    // flatters the ranked variant, whose output elements are half the width. The
    // thinned map keeps a 1-in-7 sample of the keys to land in the real range, so
    // the two together bracket the answer rather than leaning on the friendly end.
    let fill = |keep: usize| {
        let words = zeroed((c[SLOTS][k] as usize).div_ceil(64));
        moves.par_chunks(CHUNK).for_each(|chunk| {
            for (i, m) in chunk.iter().enumerate() {
                if i % keep != 0 {
                    continue;
                }
                let bit = rank(m.to_compressed_repr(), &hc, &lr);
                words[(bit >> 6) as usize].fetch_or(1u64 << (bit & 63), Ordering::Relaxed);
            }
        });
        words
    };
    let maps = [("survives ~all", fill(1)), ("survives ~15%", fill(7))];
    drop(moves);

    // what the narrowed `visited` would hold for this layer
    let probe_ranks: Vec<u32> = probe
        .iter()
        .map(|b| rank(b.to_compressed_repr(), &hc, &lr) as u32)
        .collect();

    eprintln!(
        "probe side: {} boards ({k} pegs), {} KiB as Board vs {} KiB as u32 rank, {reps} reps\n",
        probe.len(),
        probe.len() * 8 / 1024,
        probe.len() * 4 / 1024,
    );

    println!(
        "{:>24} {:>10} {:>10} {:>9}",
        "", "Board", "u32 rank", "delta"
    );
    for (label, words) in &maps {
        let mut tb = vec![];
        let mut tr = vec![];
        let (mut hits_b, mut hits_r) = (0, 0);
        for _ in 0..reps {
            let t = Instant::now();
            let kept: Vec<Board> = probe
                .par_chunks(CHUNK)
                .map(|ch| intersect_boards(words, ch, &hc, &lr))
                .reduce(Vec::new, |mut a, mut b| {
                    a.append(&mut b);
                    a
                });
            tb.push(t.elapsed().as_micros());
            hits_b = kept.len();

            let t = Instant::now();
            let kept: Vec<u32> = probe_ranks
                .par_chunks(CHUNK)
                .map(|ch| intersect_ranks(words, ch))
                .reduce(Vec::new, |mut a, mut b| {
                    a.append(&mut b);
                    a
                });
            tr.push(t.elapsed().as_micros());
            hits_r = kept.len();
        }
        assert_eq!(hits_b, hits_r, "the two filters disagree on what survives");

        let med = |v: &mut Vec<u128>| {
            v.sort();
            v[v.len() / 2] as f64 / 1000.0
        };
        let (b, r) = (med(&mut tb), med(&mut tr));
        println!(
            "{:>24} {:>9.3}ms {:>9.3}ms {:>8.1}%",
            format!(
                "{label} ({:.0}%)",
                hits_b as f64 / probe.len() as f64 * 100.
            ),
            b,
            r,
            (r / b - 1.0) * 100.0
        );
    }
    println!(
        "\nthe shrink phase runs ~16 such rounds, the later ones far smaller;\n\
         scale by round size, not by count."
    );
}
