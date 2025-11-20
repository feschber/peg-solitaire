use std::fmt::{Display, Error, Formatter};

use crate::board::Idx;

#[derive(Clone, Copy, Debug)]
pub enum Dir {
    North,
    West,
    East,
    South,
}

impl Display for Dir {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self {
            Dir::North => write!(f, "^"),
            Dir::West => write!(f, "<"),
            Dir::East => write!(f, ">"),
            Dir::South => write!(f, "v"),
        }
    }
}

impl Dir {
    pub(crate) fn mov(&self, pos: (Idx, Idx)) -> ((Idx, Idx), (Idx, Idx)) {
        let (y, x) = pos;
        match self {
            Dir::North => ((y - 1, x), (y - 2, x)),
            Dir::West => ((y, x - 1), (y, x - 2)),
            Dir::East => ((y, x + 1), (y, x + 2)),
            Dir::South => ((y + 1, x), (y + 2, x)),
        }
    }

    pub(crate) fn enumerate() -> [Self; 4] {
        [Dir::North, Dir::West, Dir::East, Dir::South]
    }
}
