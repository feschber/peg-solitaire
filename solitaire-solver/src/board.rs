use std::fmt::{Display, Formatter, Write};

use crate::{Dir, Move};
pub(crate) type Idx = i64;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Board(pub u64);

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
        let mut board = Self::full();
        board.unset((Board::SIZE / 2, Board::SIZE / 2));
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
    pub const SLOTS: usize = 33;
    pub const SIZE: Idx = 7;
    const REPR: Idx = 8;

    pub fn full() -> Self {
        let mut board = Self(0);
        for y in 0..Board::SIZE {
            for x in 0..Board::SIZE {
                let pos = (y, x);
                if Self::inbounds(pos) {
                    board.set(pos);
                }
            }
        }
        board
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
        // const SOLUTION: u64 = 1 << (3 * Board::REPR + 3);
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
        let idx = y * Board::REPR + x;
        (self.0 & (1 << idx)) != 0
    }

    #[inline(always)]
    pub fn set(&mut self, pos: (Idx, Idx)) {
        debug_assert!(!self.occupied(pos));
        let (y, x) = pos;
        let idx = y * Board::REPR + x;
        self.0 |= 1 << idx;
    }

    #[inline(always)]
    fn unset(&mut self, pos: (Idx, Idx)) {
        debug_assert!(self.occupied(pos));
        let (y, x) = pos;
        let idx = y * Board::REPR + x;
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
        for y in 0..Board::SIZE {
            for x in 0..Board::SIZE {
                if self.occupied((y, x)) {
                    for dir in Dir::enumerate() {
                        if let Some(mov) = self.get_legal_move((y, x), dir) {
                            legal_moves.push(mov);
                        }
                    }
                }
            }
        }
        legal_moves
    }

    pub fn get_legal_inverse_moves(&self) -> Vec<Move> {
        let mut legal_moves = Vec::new();
        for y in 0..Board::SIZE {
            for x in 0..Board::SIZE {
                if self.occupied((y, x)) {
                    for dir in Dir::enumerate() {
                        if let Some(mov) = self.get_legal_inverse_move((y, x), dir) {
                            legal_moves.push(mov);
                        }
                    }
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
            self.get_legal_move(pos, dir)
        } else {
            None
        }
    }

    #[inline]
    fn reverse_rows(&self) -> Self {
        // we swap twice so we dont have to shift
        let x = self.0.swap_bytes().reverse_bits();
        Self({
            let mut bytes = x.to_ne_bytes();
            for b in &mut bytes {
                *b >>= 1;
            }
            u64::from_ne_bytes(bytes)
        })
    }

    #[inline]
    fn reverse_cols(&self) -> Self {
        Self(self.0.swap_bytes() >> 8)
    }

    #[inline]
    fn rotate_180(&self) -> Self {
        let x = self.0.reverse_bits();
        let mut bytes = (x >> 8).to_ne_bytes();
        for b in &mut bytes {
            *b >>= 1;
        }
        Self(u64::from_ne_bytes(bytes))
    }

    #[inline]
    fn transpose(&self) -> Self {
        // I have 0 clue, why this works
        let mut x = self.0;
        let mut t;

        t = (x ^ (x >> 7)) & 0x00AA00AA00AA00AA;
        x = x ^ t ^ (t << 7);
        t = (x ^ (x >> 14)) & 0x0000CCCC0000CCCC;
        x = x ^ t ^ (t << 14);
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
        let anti_transpose = rotate_180.transpose();

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

#[inline(always)]
fn in_mid_section(i: Idx) -> bool {
    (2..5).contains(&i)
}

#[inline(always)]
fn in_whole_range(i: Idx) -> bool {
    (0..Board::SIZE).contains(&i)
}
