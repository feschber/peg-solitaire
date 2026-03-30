use std::{
    collections::BTreeMap,
    fmt::{Display, Formatter, Result},
};

use crate::{Board, HashSet, Move};

#[derive(Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct Solution {
    steps: [Move; 31],
    count: usize,
}

impl Solution {
    pub fn push(&mut self, mov: Move) {
        self.steps[self.count] = mov;
        self.count += 1;
    }
    pub fn pop(&mut self) -> Move {
        self.count -= 1;
        self.steps[self.count]
    }
    pub fn total(&self) -> usize {
        self.steps.len()
    }
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Display for Solution {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let steps = (0..self.steps.len())
            .map(|i| format!("{}", self.steps[i]))
            .collect::<Vec<_>>();
        write!(f, "{}", steps.join(" "))?;
        Ok(())
    }
}

impl IntoIterator for Solution {
    type Item = Move;

    type IntoIter = SolutionIter;

    fn into_iter(self) -> Self::IntoIter {
        SolutionIter { sol: self, idx: 0 }
    }
}

pub struct SolutionIter {
    sol: Solution,
    idx: usize,
}

impl Iterator for SolutionIter {
    type Item = Move;

    fn next(&mut self) -> Option<Self::Item> {
        if self.idx < self.sol.steps.len() {
            let res = self.sol.steps[self.idx];
            self.idx += 1;
            Some(res)
        } else {
            None
        }
    }
}

pub fn print_solution(solution: Solution) {
    let mut board = Board::default();
    println!("{board}");
    for mov in solution {
        board = board.mov(mov);
        println!("{mov}");
        println!("{board}");
    }
}

/// A solution is a multiset of steps (step -> count)
pub type SolutionMultiset = BTreeMap<Move, usize>;

fn from_unordered_moves(
    moves: &[Move],
    solution: &mut Solution,
    move_mask: u32,
    current_board: Board,
    feasible: &HashSet<Board>,
) -> bool {
    if move_mask == (1 << 31) - 1 {
        return true;
    }
    let mut next_moves: Vec<_> = (0..31)
        .filter(|i| move_mask & (1 << i) == 0)
        .map(|i| (i, moves[i]))
        .filter(|(_, m)| current_board.is_legal_move(m.pos, m.target).is_some())
        .map(|(i, m)| (i, m, current_board.mov(m)))
        .filter(|(_, _, b)| feasible.contains(&b.normalize()))
        .collect();
    next_moves.sort_unstable_by_key(|(_, _, b)| u64::MAX - b.0);
    next_moves.dedup();
    for (i, m, b) in next_moves {
        solution.push(m);
        if from_unordered_moves(moves, solution, move_mask | (1 << i), b, feasible) {
            return true;
        }
        solution.pop();
    }
    false
}

impl From<(SolutionMultiset, &HashSet<Board>)> for Solution {
    fn from(mf: (SolutionMultiset, &HashSet<Board>)) -> Self {
        let (mset, feasible) = mf;
        log::info!("from::<SolutionMultiset>()");
        let mut vec: Vec<_> = mset
            .into_iter()
            .flat_map(|(k, v)| std::iter::repeat(k).take(v))
            .collect();
        assert_eq!(vec.len(), 31);
        // canonicalize by sorting
        vec.sort();
        vec.reverse();
        let move_mask = 0u32;
        let mut solution = Self::default();
        let board = Board::default();
        let ass = from_unordered_moves(&vec, &mut solution, move_mask, board, &feasible);
        assert!(ass);
        solution
    }
}

impl From<Solution> for [Board; 32] {
    fn from(sol: Solution) -> Self {
        let mut board = Board::default();
        let mut boards = [Board::default(); 32];
        for (i, mov) in sol.into_iter().enumerate() {
            board = board.mov(mov);
            boards[i + 1] = board
        }
        boards
    }
}
