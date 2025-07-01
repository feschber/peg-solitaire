use std::fmt::{Display, Formatter, Result};

use crate::Move;

#[derive(Clone, Default)]
pub struct Solution {
    steps: [Move; 31],
    count: usize,
}

impl Solution {
    pub(crate) fn push(&mut self, mov: Move) {
        self.steps[self.count] = mov;
        self.count += 1;
    }
    pub(crate) fn pop(&mut self) -> Move {
        self.count -= 1;
        self.steps[self.count]
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
