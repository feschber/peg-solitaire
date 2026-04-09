use voracious_radix_sort::RadixSort;

use crate::{Board, Dir};

#[allow(unused)]
fn normalize_dedup(mut constellations: Vec<Vec<Vec<Board>>>) -> Vec<Board> {
    let res = vec![];
    for dir in Dir::enumerate() {
        for idx in 0..(Board::REPR as usize * Board::REPR as usize) {
            let constellations = &mut constellations[dir as usize][idx];
            let (unchanged, normalized) = partition_normalize(constellations);
            assert!(unchanged.is_sorted());
            normalized.voracious_sort();
            let (unchanged, normalized) = (partition_dedup(unchanged), partition_dedup(normalized));
        }
    }
    res
}

fn partition_dedup(constellations: &mut [Board]) -> (&mut [Board], &mut [Board]) {
    let mut last = 0;
    for i in 0..constellations.len() {
        let c = constellations[i];
        let n = c.normalize();
        if n == c {
            constellations.swap(last, i);
            last = i + 1;
        }
    }
    constellations.split_at_mut(last)
}

fn partition_normalize(constellations: &mut [Board]) -> (&mut [Board], &mut [Board]) {
    let mut last = 0;
    for i in 0..constellations.len() {
        let c = constellations[i];
        let n = c.normalize();
        if n == c {
            constellations.swap(last, i);
            last = i + 1;
        }
    }
    constellations.split_at_mut(last)
}
