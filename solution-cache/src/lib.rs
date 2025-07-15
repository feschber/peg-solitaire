use solitaire_solver::Board;
use std::{collections::HashSet, io::Read};

pub fn load_solutions() -> HashSet<Board> {
    // use std::time::Instant;

    // let time = Instant::now();
    println!("loading solutions");
    let mut solutions = HashSet::new();
    let compressed = include_bytes!(concat!(env!("OUT_DIR"), "/solutions.dat.br")) as &[u8];
    let mut decompressor = brotli::Decompressor::new(compressed, 4096);
    let mut buf = [0u8; 4];
    decompressor.read_exact(&mut buf).expect("value");
    let count = u32::from_le_bytes(buf);
    for _ in 0..count {
        decompressor.read_exact(&mut buf).expect("value");
        let sol = u32::from_le_bytes(buf) as u64 | 0x1_0000_0000;
        solutions.insert(Board::from_compressed_repr(sol));
    }
    while let Ok(()) = decompressor.read_exact(&mut buf) {
        let sol = u32::from_le_bytes(buf) as u64;
        solutions.insert(Board::from_compressed_repr(sol));
    }
    // println!("loading done: took {:?}", time.elapsed());
    solutions
}
