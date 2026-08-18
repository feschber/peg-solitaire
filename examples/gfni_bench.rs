//! Can GFNI collapse the symmetry primitives?
//!
//! `Board::symmetries` is built from two SWAR routines: `transpose` (three
//! delta-swap stages) and `reverse_bits_in_bytes` (three mask/shift stages, run
//! twice - once on the board, once on its transpose). Together they are most of the
//! ~50 operations a `symmetries()` costs, and it runs once per source board in
//! generation plus once per `normalize` at the bulk sites.
//!
//! Both are single GFNI instructions. `vgf2p8affineqb` applies an arbitrary 8x8
//! GF(2) matrix to each byte, so reversing the bits of every byte is one matrix; and
//! with the operands swapped - the data as the matrix, a basis vector as the input -
//! it yields the columns, which is the 8x8 bit transpose. LLVM will not invent
//! either: no idiom recognizer turns a SWAR chain into GFNI.
//!
//! The catch is that these are vector instructions and the board is a `u64` in a
//! general register, so a scalar caller pays two domain crossings. That is what this
//! measures, both one board at a time and eight at a time in one 512-bit register.

use std::time::Instant;

use rayon::prelude::*;
use solitaire_solver::Board;

#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

/// `vgf2p8affineqb` computes `dst.byte[i].bit[k] = parity(A.byte[7-k] & x.byte[i])`,
/// so the matrix row producing output bit `k` is byte `7-k`. Reversing the bits of a
/// byte therefore wants `A.byte[j] = 1<<j`.
const REVERSE_BITS: i64 = 0x8040_2010_0804_0201u64 as i64;
/// The same value used as *data* rather than as a matrix: byte `i` holding `1<<i` is
/// the basis that makes the instruction read out the input matrix's columns.
const BASIS: i64 = 0x8040_2010_0804_0201u64 as i64;

/// scalar reference, copied from `board.rs`.
fn reverse_bits_in_bytes_swar(x: u64) -> u64 {
    let x = ((x & 0xAAAAAAAAAAAAAAAA) >> 1) | ((x & 0x5555555555555555) << 1);
    let x = ((x & 0xCCCCCCCCCCCCCCCC) >> 2) | ((x & 0x3333333333333333) << 2);
    ((x & 0xF0F0F0F0F0F0F0F0) >> 4) | ((x & 0x0F0F0F0F0F0F0F0F) << 4)
}

/// scalar reference, copied from `board.rs`.
fn transpose_swar(mut x: u64) -> u64 {
    let mut t;
    t = (x ^ (x >> 7)) & 0x00AA00AA00AA00AA;
    x = x ^ t ^ (t << 7);
    t = (x ^ (x >> 14)) & 0x0000CCCC0000CCCC;
    x = x ^ t ^ (t << 14);
    t = (x ^ (x >> 28)) & 0x00000000F0F0F0F0;
    x = x ^ t ^ (t << 28);
    x
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "gfni,avx,sse2")]
unsafe fn reverse_bits_in_bytes_gfni(x: u64) -> u64 {
    let v = _mm_set_epi64x(0, x as i64);
    let m = _mm_set_epi64x(0, REVERSE_BITS);
    _mm_cvtsi128_si64(_mm_gf2p8affine_epi64_epi8::<0>(v, m)) as u64
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "gfni,avx,sse2")]
unsafe fn transpose_gfni(x: u64) -> u64 {
    // Roles swapped: the board supplies the matrix and the basis supplies the bytes,
    // so each output byte is a column of the input. That lands one bit-reversal away
    // from the transpose - `parity(S.byte[7-k] & 1<<i)` is `S.byte[7-k].bit[i]` where
    // the transpose wants `S.byte[k].bit[i]` - so reverse the bits back, which is
    // itself the same instruction. Two ops against the SWAR chain's twelve.
    let v = _mm_set_epi64x(0, BASIS);
    let m = _mm_set_epi64x(0, x as i64);
    let cols = _mm_gf2p8affine_epi64_epi8::<0>(v, m);
    let rev = _mm_gf2p8affine_epi64_epi8::<0>(cols, _mm_set_epi64x(0, REVERSE_BITS));
    _mm_cvtsi128_si64(rev) as u64
}

/// eight boards at once, which is where the domain crossing is amortized.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "gfni,avx512f,avx512bw")]
unsafe fn reverse_bits_in_bytes_gfni8(x: __m512i) -> __m512i {
    _mm512_gf2p8affine_epi64_epi8::<0>(x, _mm512_set1_epi64(REVERSE_BITS))
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "gfni,avx512f,avx512bw")]
unsafe fn transpose_gfni8(x: __m512i) -> __m512i {
    let cols = _mm512_gf2p8affine_epi64_epi8::<0>(_mm512_set1_epi64(BASIS), x);
    _mm512_gf2p8affine_epi64_epi8::<0>(cols, _mm512_set1_epi64(REVERSE_BITS))
}

fn main() {
    #[cfg(not(target_arch = "x86_64"))]
    {
        eprintln!("x86_64 only");
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        if !is_x86_feature_detected!("gfni") {
            eprintln!("no GFNI on this CPU");
            return;
        }
        let avx512 = is_x86_feature_detected!("avx512f") && is_x86_feature_detected!("avx512bw");

        // correctness first: the matrices must reproduce the SWAR routines exactly
        for i in 0..200_000u64 {
            let x = i.wrapping_mul(0x9E3779B97F4A7C15).rotate_left(17);
            unsafe {
                assert_eq!(
                    reverse_bits_in_bytes_gfni(x),
                    reverse_bits_in_bytes_swar(x),
                    "GFNI bit-reverse matrix is wrong for {x:#x}"
                );
                assert_eq!(
                    transpose_gfni(x),
                    transpose_swar(x),
                    "GFNI transpose matrix is wrong for {x:#x}"
                );
            }
        }
        eprintln!("both GFNI matrices reproduce the SWAR routines exactly\n");

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
        let raw: Vec<u64> = states.iter().map(|b| b.0).collect();
        eprintln!("{} boards\n", raw.len());

        let threads = rayon::current_num_threads();
        let chunk = raw.len().div_ceil(threads * 2);
        let mut swar = vec![];
        let mut gfni1 = vec![];
        let mut gfni8 = vec![];

        for _ in 0..reps {
            let t = Instant::now();
            let s: u64 = raw
                .par_chunks(chunk)
                .map(|c| {
                    c.iter().fold(0u64, |a, &x| {
                        a ^ transpose_swar(x) ^ reverse_bits_in_bytes_swar(x)
                    })
                })
                .sum();
            swar.push(t.elapsed().as_micros());
            std::hint::black_box(s);

            let t = Instant::now();
            let s: u64 = raw
                .par_chunks(chunk)
                .map(|c| {
                    c.iter().fold(0u64, |a, &x| unsafe {
                        a ^ transpose_gfni(x) ^ reverse_bits_in_bytes_gfni(x)
                    })
                })
                .sum();
            gfni1.push(t.elapsed().as_micros());
            std::hint::black_box(s);

            if avx512 {
                let t = Instant::now();
                let s: u64 = raw
                    .par_chunks(chunk)
                    .map(|c| {
                        let mut acc = 0u64;
                        for w in c.chunks_exact(8) {
                            unsafe {
                                let v = _mm512_loadu_si512(w.as_ptr().cast());
                                let r = _mm512_xor_si512(
                                    transpose_gfni8(v),
                                    reverse_bits_in_bytes_gfni8(v),
                                );
                                let mut out = [0u64; 8];
                                _mm512_storeu_si512(out.as_mut_ptr().cast(), r);
                                for o in out {
                                    acc ^= o;
                                }
                            }
                        }
                        acc
                    })
                    .sum();
                gfni8.push(t.elapsed().as_micros());
                std::hint::black_box(s);
            }
        }

        // The number that decides whether any of this matters: what a full
        // `symmetries()` costs in the pattern generation actually uses - one board at
        // a time, result written to an 8-element array the caller then indexes.
        let mut syms = vec![];
        for _ in 0..reps {
            let t = Instant::now();
            let s: u64 = states
                .par_chunks(chunk)
                .map(|c| {
                    c.iter().fold(0u64, |a, b| {
                        let s = b.symmetries();
                        a ^ s[0].0 ^ s[1].0 ^ s[2].0 ^ s[3].0 ^ s[4].0 ^ s[5].0 ^ s[6].0 ^ s[7].0
                    })
                })
                .sum();
            syms.push(t.elapsed().as_micros());
            std::hint::black_box(s);
        }

        let med = |v: &mut Vec<u128>| {
            v.sort();
            v[v.len() / 2] as f64 / 1000.0
        };
        let s = med(&mut swar);
        println!("{:>34} {:>10}", "", "per pass over 2.6M boards");
        println!("{:>34} {:>9.3}ms", "SWAR (transpose + bit-reverse)", s);
        let g = med(&mut gfni1);
        println!(
            "{:>34} {:>9.3}ms  {:+.1}%",
            "GFNI, one board at a time",
            g,
            (g / s - 1.0) * 100.0
        );
        if avx512 {
            let g8 = med(&mut gfni8);
            println!(
                "{:>34} {:>9.3}ms  {:+.1}%",
                "GFNI, eight at a time (zmm)",
                g8,
                (g8 / s - 1.0) * 100.0
            );
        }
        let sy = med(&mut syms);
        println!("\n{:>34} {:>9.3}ms", "Board::symmetries(), as called", sy);
        println!(
            "{:>34} {:>9.3}ms   <- whole-run cost if ~11M calls",
            "",
            sy * 11.0 / 2.6
        );
    }
}
