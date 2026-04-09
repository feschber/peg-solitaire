use crate::Board;

// somewhat effective
#[rustfmt::skip]
const PAGODA: [usize; 64] = [
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 1, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 1, 0, 1, 0, 1, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 1, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
];

fn pagoda(board: Board) -> usize {
    board.into_iter().map(|i| PAGODA[i]).sum()
}

#[allow(unused)]
fn prune_pagoda_inverse(constellations: &mut Vec<Board>) {
    let len = constellations.len();
    constellations.retain(|&b| pagoda(b.inverse()) >= pagoda(Board::solved()));
    println!(
        "pruned {} configurations ({}%)",
        len - constellations.len(),
        (len - constellations.len()) as f32 / len as f32
    );
}
