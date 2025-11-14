use super::{
    Board,
    hash::{CustomHashMap as HashMap, CustomHashSet as HashSet},
};

/// calculate the chances of winning the game by chosing possible moves at random
pub fn calculate_p_random_chance_success(feasible: Vec<Board>) -> HashMap<Board, f64> {
    let feasible: HashSet<_> = feasible.into_iter().collect();
    let mut chances = HashMap::default();
    chances.insert(Board::solved(), 1.0);
    for i in 2..=(Board::SLOTS - 1) {
        let feasible_with_i_pegs = feasible
            .iter()
            .copied()
            .filter(|b| b.count_balls() == i as u64)
            .collect::<Vec<_>>();
        for constellation in feasible_with_i_pegs {
            let legal_moves = constellation.get_legal_moves();

            // we assume each legal move has equal chance of being taken (1 / n)
            // p_success = sum(moves, P(move) * P(success | move))
            // P(success | move) = 0.0 if infeasible, else lookup
            let p_move = 1.0 / legal_moves.len() as f64;

            let mut p_success = 0.0;

            for mov in legal_moves {
                let c_new = constellation.mov(mov).normalize();
                p_success += if feasible.contains(&c_new) {
                    p_move * *chances.get(&c_new).expect("already present")
                } else {
                    p_move * 0.0
                };
            }

            chances.insert(constellation, p_success);
        }
    }
    chances
}
