use board::Idx;
use std::fmt::{Display, Error, Formatter};

use crate::{Dir, board};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Move {
    pub pos: (Idx, Idx),
    pub skip: (Idx, Idx),
    pub target: (Idx, Idx),
}

impl Move {
    fn dir(&self) -> Dir {
        match (self.pos, self.skip) {
            (a, b) if a.0 < b.0 => Dir::South,
            (a, b) if a.0 > b.0 => Dir::North,
            (a, b) if a.1 < b.1 => Dir::East,
            (a, b) if a.1 > b.1 => Dir::West,
            _ => unreachable!(),
        }
    }
}

impl Display for Move {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        write!(f, "{}{}{}", self.pos.0, self.pos.1, self.dir())?;
        Ok(())
    }
}
