use std::{
    fmt::{Display, Formatter, Write},
    hash::Hash,
    ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, Not, Shl, Shr},
};

use crate::{Dir, Move};
#[cfg(not(target_arch = "wasm32"))]
use voracious_radix_sort::peeka_sort;
use voracious_radix_sort::{
    Dispatcher, RadixKey, Radixable, dlsd_radixsort, lsd_stable_radixsort, msd_stable_radixsort,
};

pub(crate) type Idx = i64;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord)]
pub struct Board(pub u64);

pub struct U33(u64);

impl RadixKey for U33 {
    type Key = u64;
    #[inline]
    fn into_keytype(&self) -> Self::Key {
        self.0
    }
    #[inline]
    fn type_size(&self) -> usize {
        33
    }
    #[inline]
    fn usize_to_keytype(&self, item: usize) -> Self::Key {
        item as u64
    }
    #[inline]
    fn keytype_to_usize(&self, item: Self::Key) -> usize {
        item as usize
    }
    #[inline]
    fn default_key(&self) -> Self::Key {
        0
    }
    #[inline]
    fn one(&self) -> Self::Key {
        1
    }
}

impl<T: Radixable<U33>> Dispatcher<T, U33> for U33 {
    fn voracious_sort(&self, arr: &mut [T]) {
        if arr.len() <= 300 {
            arr.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        } else {
            dlsd_radixsort(arr, 8);
        }
    }
    fn voracious_stable_sort(&self, arr: &mut [T]) {
        if arr.len() <= 200 {
            arr.sort_by(|a, b| a.partial_cmp(b).unwrap());
        } else if arr.len() <= 8000 {
            msd_stable_radixsort(arr, 8);
        } else if arr.len() <= 100_000 {
            lsd_stable_radixsort(arr, 8);
        } else {
            msd_stable_radixsort(arr, 8);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn voracious_mt_sort(&self, arr: &mut [T], thread_n: usize) {
        if arr.len() <= 256 {
            arr.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        } else if arr.len() < 5_000_000_000 {
            peeka_sort(arr, 8, 650_000, thread_n);
        } else {
            // Switch to regions sort algo
            peeka_sort(arr, 8, 5_000, thread_n);
        }
    }
}

impl Radixable<U33> for Board {
    type Key = U33;
    #[inline]
    fn key(&self) -> Self::Key {
        U33(self.to_compressed_repr())
    }
}

impl BitAnd for Board {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitAnd<u64> for Board {
    type Output = Self;

    fn bitand(self, idx: u64) -> Self::Output {
        Self(self.0 & idx)
    }
}

impl BitAndAssign for Board {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0
    }
}

impl BitOr for Board {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Board {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0
    }
}

impl BitXor for Board {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self::Output {
        Self(self.0 ^ rhs.0)
    }
}

impl Not for Board {
    type Output = Self;

    fn not(self) -> Self::Output {
        Self(!self.0)
    }
}

impl Shl<u32> for Board {
    type Output = Self;

    fn shl(self, rhs: u32) -> Self::Output {
        Self(self.0 << rhs)
    }
}

impl Shr<u32> for Board {
    type Output = Self;

    fn shr(self, rhs: u32) -> Self::Output {
        Self(self.0 >> rhs)
    }
}

impl Shl<usize> for Board {
    type Output = Self;

    fn shl(self, rhs: usize) -> Self::Output {
        Self(self.0 << rhs)
    }
}

impl Shr<usize> for Board {
    type Output = Self;

    fn shr(self, rhs: usize) -> Self::Output {
        Self(self.0 >> rhs)
    }
}

impl nohash_hasher::IsEnabled for Board {}

impl Hash for Board {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        const SEED1: u64 = 0x243f6a8885a308d3;
        const SEED2: u64 = 0x13198a2e03707344;
        let x = self.0;
        let a = (x as u32) as u64 ^ SEED1;
        let b = (x >> 32_u32) ^ SEED2;
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

type Lut = [[Board; 64]; 4];
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

    #[cfg(target_arch = "x86_64")]
    pub fn to_compressed_repr(&self) -> u64 {
        const MASK: u64 = (0x7 << 2)
            | (0x7 << 10)
            | (0x7f << 16)
            | (0x7f << 24)
            | (0x7f << 32)
            | (0x7 << 42)
            | (0x7 << 50);
        unsafe { core::arch::x86_64::_pext_u64(self.0, MASK) }
    }

    #[cfg(not(target_arch = "x86_64"))]
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
            | (compressed & (0x7f << (6 + 7))) << (24 - (6 + 7))
            | (compressed & (0x7f << (6 + 14))) << (32 - (6 + 14))
            | (compressed & (0x7 << (6 + 21))) << (42 - (6 + 21))
            | (compressed & (0x7 << (6 + 21 + 3))) << (50 - (6 + 21 + 3));
        Board(board)
    }

    pub fn inverse(&self) -> Board {
        !*self & Board::full()
    }

    pub fn normalize(&self) -> Self {
        let mut symmetries = self.symmetries().into_iter();
        let mut min = symmetries.next().unwrap();
        for b in symmetries {
            if b < min {
                min = b;
            }
        }
        min
    }

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn solved() -> Self {
        Self::empty().set((3, 3))
    }

    pub const fn movable_positions(&self, dir: Dir) -> Self {
        //     o . .
        //     o . .
        // o o o o o . .
        // o o o o o . .
        // o o o o o . .
        //     o . .
        //     o . .
        const MOVABLE_EAST: Board = Board::empty()
            .set((0, 2))
            .set((1, 2))
            .set((2, 0))
            .set((2, 1))
            .set((2, 2))
            .set((2, 3))
            .set((2, 4))
            .set((3, 0))
            .set((3, 1))
            .set((3, 2))
            .set((3, 3))
            .set((3, 4))
            .set((4, 0))
            .set((4, 1))
            .set((4, 2))
            .set((4, 3))
            .set((4, 4))
            .set((5, 2))
            .set((6, 2));
        const MOVABLE_WEST: Board = MOVABLE_EAST.rotate_180();
        const MOVABLE_SOUTH: Board = MOVABLE_EAST.transpose();
        const MOVABLE_NORTH: Board = MOVABLE_WEST.transpose();
        match dir {
            Dir::North => MOVABLE_NORTH,
            Dir::East => MOVABLE_EAST,
            Dir::South => MOVABLE_SOUTH,
            Dir::West => MOVABLE_WEST,
        }
    }

    pub fn mov_pattern_mask(self, dir: Dir) -> Self {
        // mask 110 patterns in a row
        self.movable_positions(dir) & self & self.dir_shift(dir, 1) & !self.dir_shift(dir, 2)
    }

    pub fn rev_mov_pattern_mask(self, dir: Dir) -> Self {
        // mask 110 patterns in a row
        self.movable_positions(dir) & self & !self.dir_shift(dir, 1) & !self.dir_shift(dir, 2)
    }

    pub const fn count_balls(&self) -> u64 {
        self.0.count_ones() as u64
    }

    #[inline(always)]
    pub fn is_solved(&self) -> bool {
        *self == Self::solved()
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

    const fn direction_mask(idx: usize, dir: Dir) -> Self {
        match dir {
            Dir::East => Self(0b111 << idx),
            Dir::West => Self(0b111 << (idx - 2)),
            Dir::South => Self(0x010101 << idx),
            Dir::North => Self(0x010101 << (idx - 2 * Self::REPR as usize)),
        }
    }

    const fn expected_mov_pattern(idx: usize, dir: Dir) -> Self {
        match dir {
            Dir::East => Self(0b011 << idx),
            Dir::West => Self(0b110 << (idx - 2)),
            Dir::South => Self(0x000101 << idx),
            Dir::North => Self(0x010100 << (idx - 2 * Board::REPR as usize)),
        }
    }

    const fn expected_revmov_pattern(idx: usize, dir: Dir) -> Self {
        match dir {
            Dir::East => Self(0b001 << idx),
            Dir::West => Self(0b100 << (idx - 2)),
            Dir::South => Self(0x000001 << idx),
            Dir::North => Self(0x010000 << (idx - 2 * Board::REPR as usize)),
        }
    }

    const fn gen_luts() -> (Lut, Lut, Lut) {
        let mut dir_lut = [[Board(0u64); 64]; 4];
        let mut exp_mov_lut = [[Board(0u64); 64]; 4];
        let mut exp_rev_lut = [[Board(0u64); 64]; 4];
        let mut d = 0;
        while d < 4 {
            let dir = match d {
                0 => Dir::East,
                1 => Dir::West,
                2 => Dir::South,
                _ => Dir::North,
            };
            let mut i = 0;
            while i < 64 {
                dir_lut[d][i] = Self::direction_mask(i, dir);
                exp_mov_lut[d][i] = Self::expected_mov_pattern(i, dir);
                exp_rev_lut[d][i] = Self::expected_revmov_pattern(i, dir);
                i += 1;
            }
            d += 1;
        }
        (dir_lut, exp_mov_lut, exp_rev_lut)
    }

    #[allow(unused)]
    const DIR_LUT: [[Board; 64]; 4] = Self::gen_luts().0;
    #[allow(unused)]
    const EXP_MOV_LUT: [[Board; 64]; 4] = Self::gen_luts().1;
    #[allow(unused)]
    const EXP_REV_LUT: [[Board; 64]; 4] = Self::gen_luts().2;

    pub fn movable_at_no_bounds_check(self, idx: usize, dir: Dir) -> bool {
        let mask = Self::direction_mask(idx, dir);
        self & mask == Self::expected_mov_pattern(idx, dir)
    }

    pub fn reverse_movable_at_no_bounds_check(self, idx: usize, dir: Dir) -> bool {
        self & Self::direction_mask(idx, dir) == Self::expected_revmov_pattern(idx, dir)
    }

    /// Toggles the state of a move at a given index and direction.
    pub fn toggle_mov_idx_unchecked(self, idx: usize, dir: Dir) -> Board {
        self ^ Self::direction_mask(idx, dir)
    }

    pub fn dir_shift(self, dir: Dir, count: usize) -> Board {
        match dir {
            Dir::East => Board(self.0 >> count),
            Dir::West => Board(self.0 << count),
            Dir::South => Board(self.0 >> (count * Self::REPR as usize)),
            Dir::North => Board(self.0 << (count * Self::REPR as usize)),
        }
    }

    #[inline(always)]
    pub const fn occupied(&self, pos: (Idx, Idx)) -> bool {
        let (y, x) = pos;
        (self.0 & (1 << (y * Board::REPR + x))) != 0
    }

    pub const fn occupied_idx(&self, idx: usize) -> bool {
        (self.0 & (1 << idx)) != 0
    }

    #[inline(always)]
    pub const fn set(self, pos: (Idx, Idx)) -> Self {
        debug_assert!(!self.occupied(pos));
        let (y, x) = pos;
        Self(self.0 | 1 << (y * Board::REPR + x))
    }

    #[inline(always)]
    const fn unset(self, pos: (Idx, Idx)) -> Self {
        debug_assert!(self.occupied(pos));
        let (y, x) = pos;
        Self(self.0 & !(1 << (y * Board::REPR + x)))
    }

    #[inline(always)]
    pub const fn inbounds(pos: (Idx, Idx)) -> bool {
        pos.0 >= 0
            && pos.0 < Board::SIZE
            && pos.1 >= 0
            && pos.1 < Board::SIZE
            && (Board::empty().set(pos).0 & Board::full().0) != 0
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
    pub const fn reverse_rows(&self) -> Self {
        // we swap twice so we dont have to shift
        Self(self.0.swap_bytes().reverse_bits() >> 1)
    }

    #[inline]
    pub const fn reverse_cols(&self) -> Self {
        Self(self.0.swap_bytes() >> 8)
    }

    #[inline]
    pub const fn rotate_180(&self) -> Self {
        Self(self.0.reverse_bits() >> 9)
    }

    #[inline]
    const fn transpose(&self) -> Self {
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

    pub const fn symmetries(&self) -> [Self; 8] {
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
