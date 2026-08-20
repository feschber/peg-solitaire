use crate::Dir;

use super::{
    Board,
    hash::{CustomHashMap as HashMap, CustomHashSet as HashSet},
};

/// calculate the chances of winning the game by chosing possible moves at random
pub fn calculate_p_random_chance_success(feasible: impl Iterator<Item = Board>) -> HashMap<Board, f64> {
    let mut chances = HashMap::default();
    chances.insert(Board::solved(), 1.0);

    let mut index: HashSet<Board> = HashSet::default();
    let mut by_pegs: Vec<Vec<Board>> = vec![Vec::new(); Board::SLOTS + 2];
    for board in feasible {
        index.insert(board);
        by_pegs[board.count_pegs()].push(board);
    }

    for layer in by_pegs.iter().skip(2) {
        for board in layer {
            let mut buffer = [Board::empty(); MAX_MOVES];
            let successors = successors_of(*board, &mut buffer);

            // we assume each legal move has equal chance of being taken (1 / n)
            // p_success = sum(moves, P(move) * P(success | move))
            // P(success | move) = 0.0 if infeasible, else lookup
            let p_move = 1.0 / successors.len() as f64;

            let mut p_success = 0.0;

            for succ in successors {
                p_success += if index.contains(&succ) {
                    p_move * *chances.get(&succ).expect("already present")
                } else {
                    p_move * 0.0
                };
            }

            chances.insert(*board, p_success);
        }
    }
    chances
}

const MAX_MOVES: usize = 76;

fn successors_of(board: Board, buffer: &mut [Board; MAX_MOVES]) -> &[Board] {
    let syms = board.symmetries();
    let mut len = 0;
    for dir in Dir::enumerate() {
        for idx in board.mov_pattern_mask(dir) {
            debug_assert!(len < MAX_MOVES, "more than {MAX_MOVES} moves from one board");
            buffer[len] = Board::normalize_after_move(&syms, idx, dir);
            len += 1;
        }
    }
    &buffer[..len]
}
