//! Asks whether `normalize` should be done eight boards at a time.
//!
//! Every symmetry transform on this board is SWAR - shifts, masks, xors and
//! `swap_bytes` over one `u64` - and `normalize` is then a min over the eight
//! results. Nothing in that touches memory or branches, so it is exactly the shape a
//! 512-bit vector does eight of for the price of one. The scalar version cannot use
//! that width: one board is one lane.
//!
//! What it cannot tell us is whether the *call sites* are compute-bound. The two
//! that plausibly are - the inverse step and `expand_with_inverse` - walk a `Vec`
//! sequentially and do nothing else, so this measures them fairly. The move
//! generation loops also normalize, but there each normalize is interleaved with a
//! random probe into a 139 MiB map, and latency there may already be hiding this
//! work; that is not what this benchmark answers.

use std::time::Instant;

use rayon::prelude::*;
use solitaire_solver::Board;

/// derived, not transcribed - a wrong mask here would silently benchmark
/// something that is not the real inverse
const FULL: u64 = Board::full().0;

/// elementwise over 8 boards; every helper below is written this way so that LLVM
/// has a fixed-length, dependency-free loop to widen into one vector op.
type Lanes = [u64; 8];

#[inline(always)]
fn map8(x: Lanes, f: impl Fn(u64) -> u64) -> Lanes {
    let mut out = [0u64; 8];
    for i in 0..8 {
        out[i] = f(x[i]);
    }
    out
}

#[inline(always)]
fn zip8(a: Lanes, b: Lanes, f: impl Fn(u64, u64) -> u64) -> Lanes {
    let mut out = [0u64; 8];
    for i in 0..8 {
        out[i] = f(a[i], b[i]);
    }
    out
}

#[inline(always)]
fn transpose8(x: Lanes) -> Lanes {
    let mut out = x;
    for v in out.iter_mut() {
        let mut x = *v;
        let mut t;
        t = (x ^ (x >> 7)) & 0x00AA00AA00AA00AA;
        x = x ^ t ^ (t << 7);
        t = (x ^ (x >> 14)) & 0x0000CCCC0000CCCC;
        x = x ^ t ^ (t << 14);
        t = (x ^ (x >> 28)) & 0x00000000F0F0F0F0;
        x = x ^ t ^ (t << 28);
        *v = x;
    }
    out
}

#[inline(always)]
fn reverse_bits_in_bytes8(x: Lanes) -> Lanes {
    map8(x, |x| {
        let x = ((x & 0xAAAAAAAAAAAAAAAA) >> 1) | ((x & 0x5555555555555555) << 1);
        let x = ((x & 0xCCCCCCCCCCCCCCCC) >> 2) | ((x & 0x3333333333333333) << 2);
        ((x & 0xF0F0F0F0F0F0F0F0) >> 4) | ((x & 0x0F0F0F0F0F0F0F0F) << 4)
    })
}

/// the same eight symmetries `Board::symmetries` produces, reduced to their minimum
/// - eight boards at a time, in the same order so the results are bit-identical.
#[inline(always)]
fn normalize8(x: Lanes) -> Lanes {
    let transposed = transpose8(x);
    let reverse_cols = map8(x, |v| v.swap_bytes() >> 8);
    let rotate_270 = map8(transposed, |v| v.swap_bytes() >> 8);

    let pbr_self = reverse_bits_in_bytes8(x);
    let reverse_rows = map8(pbr_self, |v| v >> 1);
    let rotate_180 = map8(pbr_self, |v| v.swap_bytes() >> 9);

    let pbr_transposed = reverse_bits_in_bytes8(transposed);
    let rotate_90 = map8(pbr_transposed, |v| v >> 1);
    let anti_transpose = map8(pbr_transposed, |v| v.swap_bytes() >> 9);

    let mut min = x;
    for cand in [
        rotate_90,
        rotate_180,
        rotate_270,
        reverse_cols,
        reverse_rows,
        anti_transpose,
        transposed,
    ] {
        min = zip8(min, cand, |a, b| if b < a { b } else { a });
    }
    min
}

fn normalize_batched(boards: &mut [Board]) {
    for chunk in boards.chunks_exact_mut(8) {
        let mut lanes = [0u64; 8];
        for (l, b) in lanes.iter_mut().zip(chunk.iter()) {
            *l = b.0;
        }
        let out = normalize8(lanes);
        for (b, v) in chunk.iter_mut().zip(out) {
            b.0 = v;
        }
    }
    let n = boards.len();
    for b in &mut boards[n - n % 8..] {
        *b = b.normalize();
    }
}

/// what the inverse step does: invert, then normalize.
fn inverse_normalize_batched(boards: &mut [Board]) {
    for b in boards.iter_mut() {
        b.0 = !b.0 & FULL;
    }
    normalize_batched(boards);
}

fn main() {
    let reps: usize = std::env::var("REPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(9);

    eprintln!("building a realistic board set...");
    let mut states = vec![Board::solved()];
    loop {
        let mut next = Board::possible_reverse_moves(&states);
        Board::normalize_all(&mut next);
        next.sort_unstable_by_key(|b| b.to_compressed_repr());
        next.dedup();
        states = next;
        if states.len() > 2_000_000 {
            break;
        }
    }
    eprintln!("{} boards ({} pegs)\n", states.len(), states[0].count_pegs());

    // correctness: batched must agree with the scalar version bit for bit
    let mut a = states.clone();
    let mut b = states.clone();
    Board::normalize_all(&mut a);
    normalize_batched(&mut b);
    assert_eq!(a, b, "batched normalize disagrees with Board::normalize_all");
    let mut a = states.clone();
    let mut b = states.clone();
    a.iter_mut().for_each(|x| *x = Board(!x.0 & FULL).normalize());
    inverse_normalize_batched(&mut b);
    assert_eq!(a, b, "batched inverse+normalize disagrees");
    eprintln!("batched output is identical to scalar\n");

    let threads = rayon::current_num_threads();
    let chunk = states.len().div_ceil(threads * 2);
    let mut scalar = vec![];
    let mut batched = vec![];
    let mut scalar_inv = vec![];
    let mut batched_inv = vec![];
    for _ in 0..reps {
        let mut v = states.clone();
        let t = Instant::now();
        v.par_chunks_mut(chunk).for_each(Board::normalize_all);
        scalar.push(t.elapsed().as_micros());

        let mut v = states.clone();
        let t = Instant::now();
        v.par_chunks_mut(chunk).for_each(normalize_batched);
        batched.push(t.elapsed().as_micros());

        let mut v = states.clone();
        let t = Instant::now();
        v.par_chunks_mut(chunk)
            .for_each(|c| c.iter_mut().for_each(|x| *x = Board(!x.0 & FULL).normalize()));
        scalar_inv.push(t.elapsed().as_micros());

        let mut v = states.clone();
        let t = Instant::now();
        v.par_chunks_mut(chunk).for_each(inverse_normalize_batched);
        batched_inv.push(t.elapsed().as_micros());
    }

    // The two call sites do not just normalize - they go through `par::parallel`,
    // which maps each chunk into its own fresh `Vec` and then copies all of them
    // into one. For a map whose output size is known in advance (1:1 here, 1:2 for
    // `expand_with_inverse`) that round trip is avoidable: allocate the output once
    // and let each chunk write into its own slice of it. Same work, half the
    // allocation and none of the copy.
    let mut collect_join = vec![];
    let mut direct = vec![];
    for _ in 0..reps {
        let t = Instant::now();
        let parts: Vec<Vec<Board>> = states
            .par_chunks(chunk)
            .map(|c| c.iter().map(|x| Board(!x.0 & FULL).normalize()).collect())
            .collect();
        // `par::par_join`'s shape, not a sequential concat - it splits the output
        // into per-chunk slices and copies them in parallel, precisely because the
        // sequential version showed up as one core memmoving while the rest idled.
        let lens: Vec<usize> = parts.iter().map(|p| p.len()).collect();
        let total: usize = lens.iter().sum();
        let mut out: Vec<Board> = Vec::with_capacity(total);
        {
            let mut rest = out.spare_capacity_mut();
            let mut dsts = Vec::with_capacity(parts.len());
            for len in &lens {
                let (a, b) = rest.split_at_mut(*len);
                dsts.push(a);
                rest = b;
            }
            dsts.into_par_iter().zip(parts.par_iter()).for_each(|(dst, src)| {
                let dst: &mut [Board] = unsafe { std::mem::transmute(dst) };
                dst.copy_from_slice(src);
            });
        }
        unsafe { out.set_len(total) };
        collect_join.push(t.elapsed().as_micros());
        std::hint::black_box(&out);

        let t = Instant::now();
        let mut out: Vec<Board> = vec![Board(0); states.len()];
        out.par_chunks_mut(chunk)
            .zip(states.par_chunks(chunk))
            .for_each(|(dst, src)| {
                for (d, x) in dst.iter_mut().zip(src) {
                    *d = Board(!x.0 & FULL).normalize();
                }
            });
        direct.push(t.elapsed().as_micros());
        std::hint::black_box(&out);
    }

    let med = |v: &mut Vec<u128>| {
        v.sort();
        v[v.len() / 2] as f64 / 1000.0
    };
    println!("{:>26} {:>10} {:>10} {:>9}", "", "scalar", "8-wide", "delta");
    let (s, b) = (med(&mut scalar), med(&mut batched));
    println!("{:>26} {:>9.3}ms {:>9.3}ms {:>8.1}%", "normalize", s, b, (b / s - 1.0) * 100.0);
    let (s, b) = (med(&mut scalar_inv), med(&mut batched_inv));
    println!("{:>26} {:>9.3}ms {:>9.3}ms {:>8.1}%", "inverse + normalize", s, b, (b / s - 1.0) * 100.0);

    println!("\nwhole-step shapes (what the solver actually runs):");
    let (s, b) = (med(&mut collect_join), med(&mut direct));
    println!("{:>26} {:>9.3}ms {:>9.3}ms {:>8.1}%", "chunk-Vecs then join", s, b, (b / s - 1.0) * 100.0);
    println!("{:>26}   <- `par::parallel`      <- write into one output", "");
}
