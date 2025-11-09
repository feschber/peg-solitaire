use std::{
    fmt::{Display, Formatter, Write},
    hash::Hash,
};

use crate::{Dir, Move};
pub(crate) type Idx = i64;

#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord)]
pub struct Board(pub u64);

impl nohash_hasher::IsEnabled for Board {}

impl Hash for Board {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        const SEED1: u64 = 0x243f6a8885a308d3;
        const SEED2: u64 = 0x13198a2e03707344;
        let x = self.0;
        let a = (x as u32) as u64 ^ SEED1;
        let b = (x >> 32 as u32) as u64 ^ SEED2;
        let x: u128 = a as u128 * b as u128;
        let lo = x as u64;
        let hi = (x >> 64) as u64;
        let x = lo ^ hi;
        // x ^= x >> 30;
        // x *= SEED1;
        // x ^= x >> 27;
        // x = x * SEED2;
        // x ^= x >> 31;
        x.hash(state)
    }
}

impl Display for Board {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        for y in 0..Board::SIZE {
            for x in 0..Board::SIZE {
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
        const { Self::full().unset((Board::SIZE / 2, Board::SIZE / 2)) }
    }
}

#[test]
fn test_compression() {
    let board = Board::default().set((3, 3));
    let compressed = board.to_compressed_repr();
    assert_eq!(compressed, 0x1_ffff_ffff);
    println!("{:b}", compressed);
    println!("{:b}", board.0);
    let decompressed = Board::from_compressed_repr(compressed);
    println!("{:b}", decompressed.0);
    assert_eq!(decompressed, board);
}

impl Board {
    pub const SLOTS: usize = 33;
    pub const SIZE: Idx = 7;
    pub const REPR: Idx = 8;

    pub const fn full() -> Self {
        let mut b = Self::empty();
        b.0 |= 0x7 << 2;
        b.0 |= 0x7 << (Board::REPR + 2);
        b.0 |= 0x7f << (2 * Board::REPR);
        b.0 |= 0x7f << (3 * Board::REPR);
        b.0 |= 0x7f << (4 * Board::REPR);
        b.0 |= 0x7 << (5 * Board::REPR + 2);
        b.0 |= 0x7 << (6 * Board::REPR + 2);
        b
    }

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

    pub fn inverse(&self) -> Board {
        let full = Board::full();
        Self(!self.0 & full.0)
    }

    pub fn normalize(&self) -> Self {
        let normalized = self.symmetries().map(|s| s.0).into_iter().min().unwrap();
        Board { 0: normalized }
    }

    pub const fn empty() -> Self {
        Self { 0: 0 }
    }

    pub const fn solved() -> Self {
        let mut board = Self::empty();
        board.0 |= 1 << Board::REPR * 3 + 3;
        board
    }

    pub fn count_balls(&self) -> u64 {
        self.0.count_ones() as u64
    }

    #[inline(always)]
    pub fn is_solved(&self) -> bool {
        // exactly one bit is set
        // self.0.is_power_of_two()
        const SOLUTION: u64 = 1 << (3 * Board::REPR + 3);
        self.0 == SOLUTION
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
                let idx = y * Board::REPR + x;
                vec |= 1 << idx;
                i += 1;
            }
            vec
        };
        (self.0 & POSITION_VEC) != 0
    }

    #[inline(always)]
    pub fn mov(&self, mov: Move) -> Board {
        debug_assert!(Self::inbounds(mov.pos));
        debug_assert!(Self::inbounds(mov.skip));
        debug_assert!(Self::inbounds(mov.target));
        debug_assert!(self.occupied(mov.pos));
        debug_assert!(self.occupied(mov.skip));
        debug_assert!(!self.occupied(mov.target));
        self.unset(mov.pos).unset(mov.skip).set(mov.target)
    }

    pub fn reverse_mov(&self, mov: Move) -> Board {
        debug_assert!(Self::inbounds(mov.pos));
        debug_assert!(Self::inbounds(mov.skip));
        debug_assert!(Self::inbounds(mov.target));
        debug_assert!(!self.occupied(mov.pos));
        debug_assert!(!self.occupied(mov.skip));
        debug_assert!(self.occupied(mov.target));
        self.set(mov.pos).set(mov.skip).unset(mov.target)
    }

    #[inline(always)]
    pub const fn occupied(&self, pos: (Idx, Idx)) -> bool {
        let (y, x) = pos;
        (self.0 & (1 << y * Board::REPR + x)) != 0
    }

    #[inline(always)]
    pub const fn set(self, pos: (Idx, Idx)) -> Self {
        debug_assert!(!self.occupied(pos));
        let (y, x) = pos;
        Self(self.0 | 1 << y * Board::REPR + x)
    }

    #[inline(always)]
    const fn unset(self, pos: (Idx, Idx)) -> Self {
        debug_assert!(self.occupied(pos));
        let (y, x) = pos;
        Self(self.0 & !(1 << y * Board::REPR + x))
    }

    #[inline(always)]
    pub const fn inbounds(pos: (Idx, Idx)) -> bool {
        (Board::empty().set(pos).0 & Board::full().0) != 0
            && pos.0 >= 0
            && pos.0 < Board::SIZE
            && pos.1 >= 0
            && pos.1 < Board::SIZE
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
    pub fn get_legal_inverse_move(&self, target: (Idx, Idx), dir: Dir) -> Option<Move> {
        let (skip, pos) = dir.mov(target);
        if Self::inbounds(pos) && !self.occupied(skip) && !self.occupied(pos) {
            Some(Move { pos, skip, target })
        } else {
            None
        }
    }

    pub fn get_legal_moves(&self) -> Vec<Move> {
        let mut legal_moves = Vec::new();
        let mut copy = self.0;
        while copy != 0 {
            let idx = copy.trailing_zeros();
            let y = idx as i64 / Board::REPR;
            let x = idx as i64 % Board::REPR;
            copy &= !(1 << idx);
            for dir in Dir::enumerate() {
                if let Some(mov) = self.get_legal_move((y, x), dir) {
                    legal_moves.push(mov);
                }
            }
        }
        legal_moves
    }

    pub fn get_legal_inverse_moves(&self) -> Vec<Move> {
        let mut legal_moves = Vec::new();
        let mut copy = self.0;
        while copy != 0 {
            let idx = copy.trailing_zeros();
            let y = idx as i64 / Board::REPR;
            let x = idx as i64 % Board::REPR;
            copy &= !(1 << idx);
            for dir in Dir::enumerate() {
                if let Some(mov) = self.get_legal_inverse_move((y, x), dir) {
                    legal_moves.push(mov);
                }
            }
        }
        legal_moves
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
            self.occupied(pos);
            self.get_legal_move(pos, dir)
        } else {
            None
        }
    }

    #[inline]
    fn reverse_rows(&self) -> Self {
        // we swap twice so we dont have to shift
        Self(self.0.swap_bytes().reverse_bits() >> 1)
    }

    #[inline]
    fn reverse_cols(&self) -> Self {
        Self(self.0.swap_bytes() >> 8)
    }

    #[inline]
    fn rotate_180(&self) -> Self {
        Self(self.0.reverse_bits() >> 9)
    }

    #[inline]
    fn transpose(&self) -> Self {
        let mut x = self.0;
        let mut t;

        //    0x00AA00AA00AA00AA          0x0000CCCC0000CCCC          0x00000000F0F0F0F0
        //    -----------------------    ------------------------    ------------------------
        //    .  1  .  1  .  1  .  1      .  .  1  1  .  .  1  1      .  .  .  .  1  1  1  1
        //    .  .  .  .  .  .  .  .      .  .  1  1  .  .  1  1      .  .  .  .  1  1  1  1
        //    .  1  .  1  .  1  .  1      .  .  .  .  .  .  .  .      .  .  .  .  1  1  1  1
        //    .  .  .  .  .  .  .  .      .  .  .  .  .  .  .  .      .  .  .  .  1  1  1  1
        //    .  1  .  1  .  1  .  1      .  .  1  1  .  .  1  1      .  .  .  .  .  .  .  .
        //    .  .  .  .  .  .  .  .      .  .  1  1  .  .  1  1      .  .  .  .  .  .  .  .
        //    .  1  .  1  .  1  .  1      .  .  .  .  .  .  .  .      .  .  .  .  .  .  .  .
        //    .  .  .  .  .  .  .  .      .  .  .  .  .  .  .  .      .  .  .  .  .  .  .  .

        // transpose 2x2 submatrices
        // calculate difference between b c in [a b, c d]
        t = (x ^ (x >> 7)) & 0x00AA00AA00AA00AA;
        // xor difference to b and c
        x = x ^ t ^ (t << 7);

        // transpose 2x2 in 4x4 submatrices
        t = (x ^ (x >> 14)) & 0x0000CCCC0000CCCC;
        x = x ^ t ^ (t << 14);

        // transpose 4x4 in 8x8 matrix
        t = (x ^ (x >> 28)) & 0x00000000F0F0F0F0;
        x = x ^ t ^ (t << 28);

        Self(x)
    }

    pub fn symmetries(&self) -> [Self; 8] {
        let transposed = self.transpose();
        let reverse_cols = self.reverse_cols();
        let reverse_rows = self.reverse_rows();
        let rotate_180 = self.rotate_180();
        let rotate_90 = transposed.reverse_rows();
        let rotate_270 = transposed.reverse_cols();
        let anti_transpose = transposed.rotate_180();

        [
            *self,
            rotate_90,
            rotate_180,
            rotate_270,
            reverse_cols,
            reverse_rows,
            anti_transpose,
            transposed,
        ]
    }
}
