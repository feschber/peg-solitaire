//! Isolates the one question that decides whether ranking `DenseKeySet`'s key
//! space is worth it: does 5.4x better locality beat two extra table lookups per
//! key?
//!
//! See `keyspace_footprint.rs` for where the 5.4x comes from. This runs the real
//! key stream of the largest round through both a raw `2^33`-bit map and a ranked
//! `C(33, k)`-bit one, using the same parallel-chunked, prefetch-pipelined loop
//! shape as `feasible.rs`, and times set and test separately.
//!
//! Deliberately *not* a full port of `DenseKeySet`: no summary index, since it is
//! identical work in both variants (and would in fact shrink with the ranked map
//! too, so leaving it out is the conservative choice for the ranked side).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use rayon::prelude::*;
use solitaire_solver::Board;

const SLOTS: usize = 33;
const LOW_BITS: u32 = 16;
const LOW_MASK: u64 = (1 << LOW_BITS) - 1;
/// keys per work unit, mirroring `generate_into_bitset`'s per-chunk parallelism
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
    assert_eq!(c[33][2], 528);
    assert_eq!(c[33][16], 1_166_803_110);
    assert_eq!((0..=33).map(|k| c[33][k]).sum::<u64>(), 1u64 << 33);
    c
}

/// `low_rank[l]` = how many 16-bit values below `l` share its popcount.
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

/// `high_cum[h]` = how many popcount-`k` keys lie below the prefix `h << LOW_BITS`.
fn high_cum_table(k: usize, c: &[Vec<u64>]) -> Vec<u32> {
    let highs = 1usize << (SLOTS as u32 - LOW_BITS);
    let mut t = vec![0u32; highs];
    let mut acc = 0u64;
    for (h, slot) in t.iter_mut().enumerate() {
        *slot = acc as u32;
        // keys with this exact prefix: the low half must supply the rest of the pegs
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

/// the O(k) colex rank, as an independent check on the two-table form
fn colex_rank(mut key: u64, c: &[Vec<u64>]) -> u64 {
    let mut r = 0;
    let mut i = 1;
    while key != 0 {
        r += c[key.trailing_zeros() as usize][i];
        key &= key - 1;
        i += 1;
    }
    r
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

/// sets every key, pipelined `PREFETCH_DISTANCE` ahead, in parallel chunks.
/// `index` maps a key to its bit position, which is where the two variants differ.
fn set_all(words: &[AtomicU64], keys: &[u64], index: impl Fn(u64) -> u64 + Send + Sync) -> u128 {
    let t = Instant::now();
    keys.par_chunks(CHUNK).for_each(|chunk| {
        let ahead = chunk.get(PREFETCH_DISTANCE..).unwrap_or(&[]);
        for key in chunk.iter().take(PREFETCH_DISTANCE) {
            prefetch(words, (index(*key) >> 6) as usize);
        }
        let split = chunk.len().saturating_sub(PREFETCH_DISTANCE);
        for (key, nxt) in chunk[..split].iter().zip(ahead) {
            prefetch(words, (index(*nxt) >> 6) as usize);
            let bit = index(*key);
            words[(bit >> 6) as usize].fetch_or(1u64 << (bit & 63), Ordering::Relaxed);
        }
        for key in &chunk[split..] {
            let bit = index(*key);
            words[(bit >> 6) as usize].fetch_or(1u64 << (bit & 63), Ordering::Relaxed);
        }
    });
    t.elapsed().as_micros()
}

fn test_all(
    words: &[AtomicU64],
    keys: &[u64],
    index: impl Fn(u64) -> u64 + Send + Sync,
) -> (u128, usize) {
    let t = Instant::now();
    let hits: usize = keys
        .par_chunks(CHUNK)
        .map(|chunk| {
            let ahead = chunk.get(PREFETCH_DISTANCE..).unwrap_or(&[]);
            let mut n = 0;
            for key in chunk.iter().take(PREFETCH_DISTANCE) {
                prefetch(words, (index(*key) >> 6) as usize);
            }
            let split = chunk.len().saturating_sub(PREFETCH_DISTANCE);
            for (key, nxt) in chunk[..split].iter().zip(ahead) {
                prefetch(words, (index(*nxt) >> 6) as usize);
                let bit = index(*key);
                n += ((words[(bit >> 6) as usize].load(Ordering::Relaxed) >> (bit & 63)) & 1)
                    as usize;
            }
            for key in &chunk[split..] {
                let bit = index(*key);
                n += ((words[(bit >> 6) as usize].load(Ordering::Relaxed) >> (bit & 63)) & 1)
                    as usize;
            }
            n
        })
        .sum();
    (t.elapsed().as_micros(), hits)
}

fn popcount(words: &[AtomicU64]) -> u64 {
    words
        .par_iter()
        .map(|w| w.load(Ordering::Relaxed).count_ones() as u64)
        .sum()
}

fn main() {
    let c = binomials();
    let reps: usize = std::env::var("REPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    // rebuild the growth phase up to the largest round, as feasible.rs would
    eprintln!("building rounds...");
    let mut states = vec![Board::solved()];
    // `prev` is the round before the last, which is what the real shrink round
    // intersects against, so it is the natural probe set
    let prev = loop {
        let mut next = Board::possible_reverse_moves(&states);
        Board::normalize_all(&mut next);
        next.sort_unstable_by_key(|b| b.to_compressed_repr());
        next.dedup();
        let prev = std::mem::replace(&mut states, next);
        if states.len() > 2_400_000 {
            break prev;
        }
    };
    let k_src = states[0].count_pegs();
    // the keys this round would SET: every forward move of every board, normalized,
    // duplicates and all - that is the real stream generate_into_bitset sees
    let mut moves = Board::possible_moves(&states);
    Board::normalize_all(&mut moves);
    let k = moves[0].count_pegs();
    assert!(moves.iter().all(|b| b.count_pegs() == k));
    let set_keys: Vec<u64> = moves.iter().map(|b| b.to_compressed_repr()).collect();
    drop(moves);
    // the keys it would TEST: the smaller growth-phase side it intersects against
    let probe_keys: Vec<u64> = prev.iter().map(|b| b.to_compressed_repr()).collect();

    let high_cum = high_cum_table(k, &c);
    let low_rank = low_rank_table();

    // the two-table rank must agree with the independent colex form, and stay in range
    for &key in set_keys.iter().step_by(set_keys.len() / 1000 + 1) {
        let r = rank(key, &high_cum, &low_rank);
        assert_eq!(
            r,
            colex_rank(key, &c),
            "two-table rank disagrees for {key:#x}"
        );
        assert!(r < c[SLOTS][k]);
    }

    let raw_words = 1usize << (SLOTS - 6);
    let ranked_words = (c[SLOTS][k] as usize).div_ceil(64);
    eprintln!(
        "source round: {} boards ({k_src} pegs) -> {} moves ({k} pegs), {} probes\n\
         raw map {} MiB, ranked map {} MiB ({:.2}x smaller), {reps} timed reps\n",
        states.len(),
        set_keys.len(),
        probe_keys.len(),
        raw_words * 8 / (1 << 20),
        ranked_words * 8 / (1 << 20),
        raw_words as f64 / ranked_words as f64,
    );

    let raw = zeroed(raw_words);
    let ranked = zeroed(ranked_words);
    let id = |key: u64| key;
    let rk = |key: u64| rank(key, &high_cum, &low_rank);

    // cold pass: includes first-touch page faults, which is a one-time cost in the
    // real solver (the map is allocated once and reused) but worth seeing separately
    let cold_raw = set_all(&raw, &set_keys, id);
    let cold_ranked = set_all(&ranked, &set_keys, rk);

    // both maps must now hold exactly the same set of distinct keys
    let (praw, prank) = (popcount(&raw), popcount(&ranked));
    assert_eq!(
        praw, prank,
        "the two maps disagree on the distinct key count"
    );
    eprintln!("distinct keys: {praw} (both maps agree)\n");

    let mut sr = vec![];
    let mut sk = vec![];
    let mut tr = vec![];
    let mut tk = vec![];
    for _ in 0..reps {
        sr.push(set_all(&raw, &set_keys, id));
        sk.push(set_all(&ranked, &set_keys, rk));
        let (a, ha) = test_all(&raw, &probe_keys, id);
        let (b, hb) = test_all(&ranked, &probe_keys, rk);
        assert_eq!(ha, hb, "the two maps disagree on hit count");
        tr.push(a);
        tk.push(b);
    }
    let med = |v: &mut Vec<u128>| {
        v.sort();
        v[v.len() / 2] as f64 / 1000.0
    };
    println!("{:>28} {:>10} {:>10} {:>9}", "", "raw", "ranked", "delta");
    println!(
        "{:>28} {:>9.2}ms {:>9.2}ms {:>8.1}%",
        "set, cold (with faults)",
        cold_raw as f64 / 1000.0,
        cold_ranked as f64 / 1000.0,
        (cold_ranked as f64 / cold_raw as f64 - 1.0) * 100.0
    );
    let (a, b) = (med(&mut sr), med(&mut sk));
    println!(
        "{:>28} {:>9.2}ms {:>9.2}ms {:>8.1}%",
        "set, warm",
        a,
        b,
        (b / a - 1.0) * 100.0
    );
    let (a, b) = (med(&mut tr), med(&mut tk));
    println!(
        "{:>28} {:>9.2}ms {:>9.2}ms {:>8.1}%",
        "test, warm",
        a,
        b,
        (b / a - 1.0) * 100.0
    );
}
