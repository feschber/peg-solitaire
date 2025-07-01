use std::fmt::{Display, Formatter, Write};

use crate::{Dir, Move};
pub(crate) type Idx = i64;

pub(crate) const BOARD_SIZE: Idx = 7;
const BOARD_REPR: Idx = 8;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Board(pub u64);

impl Display for Board {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        for y in 0..BOARD_SIZE {
            for x in 0..BOARD_SIZE {
                let occupied = self.occupied((y, x));
                let inbounds = Self::inbounds((y, x));
                let c = match (occupied, inbounds) {
                    (_, false) => ' ',
                    (true, _) => 'o',
                    (false, _) => '.',
                };
                f.write_char(' ')?;
                f.write_char(c)?;
                f.write_char(' ')?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

impl Default for Board {
    fn default() -> Self {
        let mut board = Self(0);
        for y in 0..BOARD_SIZE {
            for x in 0..BOARD_SIZE {
                let pos = (y, x);
                if Self::inbounds(pos) {
                    board.set(pos);
                }
            }
        }
        board.unset((BOARD_SIZE / 2, BOARD_SIZE / 2));
        board
    }
}

impl Board {
    pub(crate) fn normalize(&self) -> Self {
        let normalized = self.symmetries().map(|s| s.0).into_iter().min().unwrap();
        Board { 0: normalized }
    }

    pub fn empty() -> Self {
        Self { 0: 0 }
    }

    pub(crate) fn count_balls(&self) -> u64 {
        self.0.count_ones() as u64
    }

    #[inline(always)]
    pub(crate) fn is_solved(&self) -> bool {
        // exactly one bit is set
        // self.0.is_power_of_two()
        const SOLUTION: u64 = 1 << (3 * BOARD_REPR + 3);
        self.0 == SOLUTION
    }

    /// the game is not solvable, if none of the marked fields contain a ball:
    ///  ```
    ///        .  .  .
    ///        .  x  .
    ///  .  .  .  .  .  .  .
    ///  .  x  .  x  .  x  .
    ///  .  .  .  .  .  .  .
    ///        .  x  .
    ///        .  .  .
    /// ```
    pub(crate) fn is_solvable(&self) -> bool {
        const POSITION_VEC: u64 = {
            let mut vec = 0;
            const POSITIONS: [(Idx, Idx); 5] = [(1, 3), (3, 1), (3, 3), (3, 5), (5, 3)];
            let mut i = 0;
            while i < POSITIONS.len() {
                let (y, x) = POSITIONS[i];
                let idx = y * BOARD_REPR + x;
                vec |= 1 << idx;
                i += 1;
            }
            vec
        };
        (self.0 & POSITION_VEC) != 0
    }

    #[inline(always)]
    pub fn mov(&self, mov: Move) -> Board {
        let mut board = *self;
        debug_assert!(Self::inbounds(mov.pos));
        debug_assert!(Self::inbounds(mov.skip));
        debug_assert!(Self::inbounds(mov.target));
        debug_assert!(self.occupied(mov.pos));
        debug_assert!(self.occupied(mov.skip));
        debug_assert!(!self.occupied(mov.target));
        board.unset(mov.pos);
        board.unset(mov.skip);
        board.set(mov.target);
        board
    }

    #[inline(always)]
    pub fn occupied(&self, pos: (Idx, Idx)) -> bool {
        let (y, x) = pos;
        let idx = y * BOARD_REPR + x;
        (self.0 & (1 << idx)) != 0
    }

    #[inline(always)]
    pub fn set(&mut self, pos: (Idx, Idx)) {
        debug_assert!(!self.occupied(pos));
        let (y, x) = pos;
        let idx = y * BOARD_REPR + x;
        self.0 |= 1 << idx;
    }

    #[inline(always)]
    fn unset(&mut self, pos: (Idx, Idx)) {
        debug_assert!(self.occupied(pos));
        let (y, x) = pos;
        let idx = y * BOARD_REPR + x;
        self.0 &= !(1 << idx);
    }

    #[inline(always)]
    pub fn inbounds(pos: (Idx, Idx)) -> bool {
        let (y, x) = pos;
        in_mid_section(x) && in_whole_range(y) || in_mid_section(y) && in_whole_range(x)
    }

    #[inline(always)]
    pub(crate) fn get_legal_move(&self, pos: (Idx, Idx), dir: Dir) -> Option<Move> {
        debug_assert!(Self::inbounds(pos));
        let (skip, target) = dir.mov(pos);
        println!(
            "{:?}, {:?}, {:?}, {}, {}, {}",
            pos,
            skip,
            target,
            Self::inbounds(target),
            self.occupied(skip),
            !self.occupied(target)
        );
        if Self::inbounds(target) && self.occupied(skip) && !self.occupied(target) {
            Some(Move { pos, skip, target })
        } else {
            None
        }
    }

    pub fn is_legal_move(&self, pos: (Idx, Idx), dst: (Idx, Idx)) -> Option<Move> {
        let dist_y = (pos.0 - dst.0).abs();
        let dist_x = (pos.1 - dst.1).abs();
        if dist_y == 2 && dist_x == 0 || dist_x == 2 && dist_y == 0 {
            let dir = match (pos, dst) {
                (p, d) if d.0 < p.0 => Dir::North,
                (p, d) if d.0 > p.0 => Dir::South,
                (p, d) if d.1 < p.1 => Dir::West,
                (p, d) if d.1 > p.1 => Dir::East,
                _ => unreachable!(),
            };
            self.get_legal_move(pos, dir)
        } else {
            None
        }
    }

    pub(crate) fn rotate270(&self) -> Self {
        let mut clone = Self::empty();
        for i in 0..BOARD_SIZE {
            for j in 0..BOARD_SIZE {
                if self.occupied((i, j)) {
                    clone.set((BOARD_SIZE - 1 - j, i))
                }
            }
        }
        clone
    }

    pub(crate) fn rotate90(&self) -> Self {
        let mut clone = Self::empty();
        for i in 0..BOARD_SIZE {
            for j in 0..BOARD_SIZE {
                if self.occupied((i, j)) {
                    clone.set((j, BOARD_SIZE - 1 - i))
                }
            }
        }
        clone
    }

    pub(crate) fn rotate180(&self) -> Self {
        let mut clone = Self::empty();
        for i in 0..BOARD_SIZE {
            for j in 0..BOARD_SIZE {
                if self.occupied((i, j)) {
                    clone.set((BOARD_SIZE - 1 - i, BOARD_SIZE - 1 - j))
                }
            }
        }
        clone
    }

    pub(crate) fn mirror_horizontal(&self) -> Self {
        let mut clone = Self::empty();
        for i in 0..BOARD_SIZE {
            for j in 0..BOARD_SIZE {
                if self.occupied((i, j)) {
                    clone.set((i, BOARD_SIZE - 1 - j))
                }
            }
        }
        clone
    }

    pub(crate) fn mirror_vertical(&self) -> Self {
        let mut clone = Self::empty();
        for i in 0..BOARD_SIZE {
            for j in 0..BOARD_SIZE {
                if self.occupied((i, j)) {
                    clone.set((BOARD_SIZE - 1 - i, j))
                }
            }
        }
        clone
    }

    pub(crate) fn mirror_bl_tr(&self) -> Self {
        let mut clone = Self::empty();
        for i in 0..BOARD_SIZE {
            for j in 0..BOARD_SIZE {
                if self.occupied((i, j)) {
                    clone.set((BOARD_SIZE - 1 - j, BOARD_SIZE - 1 - i))
                }
            }
        }
        clone
    }

    pub(crate) fn mirror_tl_br(&self) -> Self {
        let mut clone = Self::empty();
        for i in 0..BOARD_SIZE {
            for j in 0..BOARD_SIZE {
                if self.occupied((i, j)) {
                    clone.set((j, i))
                }
            }
        }
        clone
    }

    pub(crate) fn symmetries(&self) -> [Self; 8] {
        let mut arr = [self.clone(); 8];
        arr[0] = *self;
        arr[1] = self.rotate90();
        arr[2] = self.rotate180();
        arr[3] = self.rotate270();
        arr[4] = self.mirror_vertical();
        arr[5] = self.mirror_horizontal();
        arr[6] = self.mirror_bl_tr();
        arr[7] = self.mirror_tl_br();
        arr
    }
}

#[inline(always)]
fn in_mid_section(i: Idx) -> bool {
    (2..5).contains(&i)
}

#[inline(always)]
fn in_whole_range(i: Idx) -> bool {
    (0..BOARD_SIZE).contains(&i)
}
