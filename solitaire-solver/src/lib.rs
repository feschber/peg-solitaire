mod board;
mod calc_first;
mod calc_naive;
mod calc_success;
mod dir;
pub mod dominators;
mod feasible;
mod hash;
mod keyset;
mod mov;
mod normalize_dedup;
mod pagoda;
mod par;
mod solution;
mod sort;
mod timer;
mod unique_solutions;

pub use board::{Board, Idx};
pub use dir::Dir;
pub use hash::{CustomHashMap as HashMap, CustomHashSet as HashSet};
pub use mov::Move;
pub use solution::{Solution, SolutionMultiset};

pub use calc_first::calculate_first_solution;
pub use calc_naive::calculate_all_solutions_naive;
pub use calc_success::calculate_p_random_chance_success;
pub use feasible::calculate_feasible_set;
pub use solution::print_solution;
pub use unique_solutions::{
    JumpMap, NOT_REMOVED, all_unique_board_paths, all_unique_jump_maps, all_unique_paths,
    all_unique_solutions,
};
