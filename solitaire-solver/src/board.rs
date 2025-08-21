use std::fmt::{Display, Formatter, Write};

use crate::{Dir, Move};
pub(crate) type Idx = i64;

pub const BOARD_SIZE: Idx = 7;
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

#[test]
fn test_compression() {
    let mut board = Board::default();
    board.set((3, 3));
    let compressed = board.to_compressed_repr();
    assert_eq!(compressed, 0x1_ffff_ffff);
    println!("{:b}", compressed);
    println!("{:b}", board.0);
    let decompressed = Board::from_compressed_repr(compressed);
    println!("{:b}", decompressed.0);
    assert_eq!(decompressed, board);
}

impl Board {
    pub fn to_compressed_repr(&self) -> u64 {
        let board = self.0;
        (board & (0x7 << 2)) >> 2
            | (board & (0x7 << 10)) >> (10 - 3)
            | (board & (0x7f << 16)) >> (16 - 6)
            | (board & (0x7f << 24)) >> (24 - (6 + 7))
            | (board & (0x7f << 32)) >> (32 - (6 + 14))
            | (board & (0x7 << 42)) >> (42 - (6 + 21))
            | (board & (0x7 << 50)) >> (50 - (6 + 21 + 3))
    }

    pub fn from_compressed_repr(compressed: u64) -> Self {
        let board = (compressed & 0x7) << 2
            | (compressed & (0x7 << 3)) << (8 + 2 - 3)
            | (compressed & (0x7f << 6)) << (16 - 6)
            | (compressed & (0x7f << 6 + 7)) << (24 - (6 + 7))
            | (compressed & (0x7f << 6 + 14)) << (32 - (6 + 14))
            | (compressed & (0x7 << 6 + 21)) << (42 - (6 + 21))
            | (compressed & (0x7 << 6 + 21 + 3)) << (50 - (6 + 21 + 3));
        Board(board)
    }

    pub(crate) fn normalize(&self) -> Self {
        let normalized = self.symmetries().map(|s| s.0).into_iter().min().unwrap();
        Board { 0: normalized }
    }

    pub fn empty() -> Self {
        Self { 0: 0 }
    }

    pub fn solved() -> Self {
        let mut board = Self::empty();
        board.set((3, 3));
        board
    }

    pub fn count_balls(&self) -> u64 {
        self.0.count_ones() as u64
    }

    #[inline(always)]
    pub fn is_solved(&self) -> bool {
        // exactly one bit is set
        self.0.is_power_of_two()
        // const SOLUTION: u64 = 1 << (3 * BOARD_REPR + 3);
        // self.0 == SOLUTION
    }

    /// the game is not solvable, if none of the marked fields contain a ball:
    ///
    ///  ```no_run
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

    pub fn reverse_mov(&self, mov: Move) -> Board {
        let mut board = *self;
        debug_assert!(Self::inbounds(mov.pos));
        debug_assert!(Self::inbounds(mov.skip));
        debug_assert!(Self::inbounds(mov.target));
        debug_assert!(!self.occupied(mov.pos));
        debug_assert!(!self.occupied(mov.skip));
        debug_assert!(self.occupied(mov.target));
        board.set(mov.pos);
        board.set(mov.skip);
        board.unset(mov.target);
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
    pub fn get_legal_move(&self, pos: (Idx, Idx), dir: Dir) -> Option<Move> {
        debug_assert!(Self::inbounds(pos));
        let (skip, target) = dir.mov(pos);
        if Self::inbounds(target) && self.occupied(skip) && !self.occupied(target) {
            Some(Move { pos, skip, target })
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn get_legal_inverse_move(&self, pos: (Idx, Idx), dir: Dir) -> Option<Move> {
        let (skip, target) = dir.mov(pos);
        if Self::inbounds(target) && !self.occupied(skip) && self.occupied(target) {
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

    pub fn symmetries(&self) -> [Self; 8] {
        let mut arr = [self.clone(); 8];
        let mut rotate_90 = Self::empty();
        let mut rotate_180 = Self::empty();
        let mut rotate_270 = Self::empty();
        let mut mirror_vertically = Self::empty();
        let mut mirror_horizontally = Self::empty();
        let mut mirror_bl_tr = Self::empty();
        let mut mirror_tl_br = Self::empty();

        for i in 0..BOARD_SIZE {
            for j in 0..BOARD_SIZE {
                if self.occupied((i, j)) {
                    rotate_90.set((j, BOARD_SIZE - 1 - i));
                    rotate_180.set((BOARD_SIZE - 1 - i, BOARD_SIZE - 1 - j));
                    rotate_270.set((BOARD_SIZE - 1 - j, i));
                    mirror_vertically.set((BOARD_SIZE - 1 - i, j));
                    mirror_horizontally.set((i, BOARD_SIZE - 1 - j));
                    mirror_bl_tr.set((BOARD_SIZE - 1 - j, BOARD_SIZE - 1 - i));
                    mirror_tl_br.set((j, i));
                }
            }
        }
        arr[0] = *self;
        arr[1] = rotate_90;
        arr[2] = rotate_180;
        arr[3] = rotate_270;
        arr[4] = mirror_vertically;
        arr[5] = mirror_horizontally;
        arr[6] = mirror_bl_tr;
        arr[7] = mirror_tl_br;
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
