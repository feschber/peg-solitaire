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

    /// Dense index for direction-keyed lookup tables; inverse of
    /// [`Dir::from_index`]. Kept as a pair so the tables and their lookups cannot
    /// disagree about the ordering.
    pub(crate) const fn index(self) -> usize {
        match self {
            Dir::North => 0,
            Dir::West => 1,
            Dir::East => 2,
            Dir::South => 3,
        }
    }

    /// Inverse of [`Dir::index`]. Panics outside `0..4`, which is unreachable for
    /// the const-evaluated table construction that uses it.
    pub(crate) const fn from_index(index: usize) -> Self {
        match index {
            0 => Dir::North,
            1 => Dir::West,
            2 => Dir::East,
            3 => Dir::South,
            _ => panic!("direction index out of range"),
        }
    }
}
