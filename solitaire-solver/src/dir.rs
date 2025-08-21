use std::fmt::{Display, Error, Formatter};

use crate::board::Idx;

#[derive(Clone, Copy, Debug)]
pub enum Dir {
    North,
    East,
    South,
    West,
}

impl Display for Dir {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            Dir::North => write!(f, "^"),
            Dir::East => write!(f, ">"),
            Dir::South => write!(f, "v"),
            Dir::West => write!(f, "<"),
        }
    }
}

impl Dir {
    pub(crate) fn mov(&self, pos: (Idx, Idx)) -> ((Idx, Idx), (Idx, Idx)) {
        let (y, x) = pos;
        match self {
            Dir::North => ((y - 1, x), (y - 2, x)),
            Dir::South => ((y + 1, x), (y + 2, x)),
            Dir::West => ((y, x - 1), (y, x - 2)),
            Dir::East => ((y, x + 1), (y, x + 2)),
        }
    }

    pub(crate) fn enumerate() -> [Self; 4] {
        [Dir::North, Dir::East, Dir::South, Dir::West]
    }
}
