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

pub type Idx = i8;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord)]
pub struct Board(pub u64);

/// One move's eight symmetry-transformed masks, for [`Board::SYM_DIR_LUT`].
///
/// `align(64)` is the point of the wrapper: eight `Board`s are exactly one cache
/// line, and forcing the alignment makes each entry occupy one rather than
/// straddling two. Without it the array would only be 8-aligned and half the
/// lookups would touch two lines.
#[repr(align(64))]
#[derive(Debug, Clone, Copy)]
pub(crate) struct SymMasks(pub(crate) [Board; 8]);

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
            peeka_sort(arr, 8, peeka_block_size(arr.len(), thread_n), thread_n);
        } else {
            // Switch to regions sort algo
            peeka_sort(arr, 8, 5_000, thread_n);
        }
    }
}

/// Block size handed to `peeka_sort` as its `blocks_info` argument.
///
/// For any array smaller than 5e9 elements - i.e. every array this solver ever
/// sorts - `peeka_sort_rec` assigns `blocks_info` straight to its `block_size`,
/// and then only parallelizes its local-sorting phase when
/// `arr.len() > block_size`; otherwise it does one `get_histogram` + `ska_swap`
/// over the whole array on the calling thread. So this value is really "how many
/// elements each core gets", and `len / block_size` is the width of the only
/// parallel phase.
///
/// A single fixed value cannot serve both ends of the range this solver sorts.
/// The upstream default of 650_000 is far too coarse for the ~1-2.4M arrays that
/// roughly ten BFS rounds produce: measured sort-bucket scaling from 1 to 16
/// threads was 1.9x at 0.99M elements, 2.9x at 1.52M, 3.5x at 2.03M and 2.38M -
/// exactly the 2/3/4/4 blocks 650_000 yields - while comparable parallel work in
/// the same rounds reached 4-8.6x. But simply lowering it to a fixed 100_000
/// measurably *regressed* the two largest rounds (+3.5ms each), where 650_000
/// already produced enough blocks and the extra ones only added histogram-merge
/// work in the (largely serial) graph-construction phase.
///
/// So scale it with the array: aim for about one block per worker, and never go
/// below `peeka_sort`'s own serial-fallback threshold of 128_000, beneath which
/// extra blocks buy no parallelism and only add merge overhead.
///
/// Both knobs are overridable (`PEG_PEEKA_BLOCKS_PER_THREAD`, `PEG_PEEKA_MIN_BLOCK`)
/// for tuning sweeps. `blocks_info` is a plain runtime argument to `peeka_sort`
/// either way, so reading these from the environment cannot change that
/// function's codegen - a sweep measures exactly what hardcoded values would do.
#[cfg(not(target_arch = "wasm32"))]
fn peeka_block_size(len: usize, thread_n: usize) -> usize {
    fn env_or(var: &str, default: usize) -> usize {
        std::env::var(var)
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&v| v > 0)
            .unwrap_or(default)
    }
    use std::sync::OnceLock;
    static BLOCKS_PER_THREAD: OnceLock<usize> = OnceLock::new();
    static MIN_BLOCK: OnceLock<usize> = OnceLock::new();
    let blocks_per_thread =
        *BLOCKS_PER_THREAD.get_or_init(|| env_or("PEG_PEEKA_BLOCKS_PER_THREAD", 1));
    let min_block = *MIN_BLOCK.get_or_init(|| env_or("PEG_PEEKA_MIN_BLOCK", 128_000));

    let target_blocks = thread_n.max(1) * blocks_per_thread;
    (len / target_blocks).max(min_block)
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
fn test_reverse_rows_rotate_180_bit_trick() {
    // reverse_rows/rotate_180 were rewritten to share a cheaper "reverse bits
    // within each byte" helper instead of each calling the full 64-bit
    // reverse_bits(). Verify the new formulas agree with the original,
    // straightforward ones for arbitrary u64 patterns (not just valid boards).
    fn reverse_rows_orig(x: u64) -> u64 {
        x.swap_bytes().reverse_bits() >> 1
    }
    fn rotate_180_orig(x: u64) -> u64 {
        x.reverse_bits() >> 9
    }
    let samples = [
        0u64,
        u64::MAX,
        0x1,
        0x8000_0000_0000_0000,
        0xAAAA_AAAA_AAAA_AAAA,
    ]
    .into_iter()
    .chain((0..100_000).map(|_| rand::random::<u64>()));
    for x in samples {
        let board = Board(x);
        assert_eq!(
            board.reverse_rows().0,
            reverse_rows_orig(x),
            "reverse_rows mismatch for {x:#x}"
        );
        assert_eq!(
            board.rotate_180().0,
            rotate_180_orig(x),
            "rotate_180 mismatch for {x:#x}"
        );
    }
}

#[test]
fn test_compressed_repr_matches_portable() {
    // `to_compressed_repr` has two implementations selected by target feature (a
    // BMI2 `pext` and a portable shift/mask chain) and only one is ever compiled,
    // so neither can be diffed against the other directly. Pin whichever is built
    // against an independent reference: extract the in-play cells one at a time.
    fn reference(board: u64) -> u64 {
        const MASK: u64 = (0x7 << 2)
            | (0x7 << 10)
            | (0x7f << 16)
            | (0x7f << 24)
            | (0x7f << 32)
            | (0x7 << 42)
            | (0x7 << 50);
        let mut out = 0u64;
        let mut out_bit = 0;
        for bit in 0..64 {
            if MASK >> bit & 1 == 1 {
                out |= (board >> bit & 1) << out_bit;
                out_bit += 1;
            }
        }
        out
    }
    // valid boards first, then arbitrary bit patterns (the mask must ignore the
    // out-of-play bits either way)
    let full = Board::full().0;
    let samples = [0u64, full, Board::default().0, Board::solved().0]
        .into_iter()
        .chain((0..50_000).map(|_| rand::random::<u64>() & full))
        .chain((0..50_000).map(|_| rand::random::<u64>()));
    for raw in samples {
        let board = Board(raw);
        assert_eq!(
            board.to_compressed_repr(),
            reference(raw),
            "compressed repr mismatch for {raw:#018x}"
        );
    }
    // and the round trip holds for valid boards
    for _ in 0..50_000 {
        let board = Board(rand::random::<u64>() & full);
        assert_eq!(
            Board::from_compressed_repr(board.to_compressed_repr()),
            board
        );
    }
}

/// Pins what `keyset.rs` relies on when it ranks keys inside the invariant subspace:
/// every move preserves both GF(4) invariants, and start and solved agree on their value.
///
/// The move half is exhaustive rather than sampled, and cheaply so: a move is an XOR with a
/// three-cell mask and the invariant is GF(2)-linear, so a move preserves it exactly when the
/// mask's own state is zero. Checking every mask therefore covers every board any sequence of
/// moves can reach. `examples/hamming_neighbors.rs` carries the same check for the eight
/// symmetries, which needs the kernel basis and is too much machinery for here.
#[test]
fn test_invariant_target_matches_start_and_solved() {
    let start = Board(Board::full().0 & !Board::solved().0);
    assert_eq!(
        Board::invariant_state(start.to_compressed_repr()),
        Board::INVARIANT_TARGET,
        "the start board must carry the target invariant"
    );
    assert_eq!(
        Board::invariant_state(Board::solved().to_compressed_repr()),
        Board::INVARIANT_TARGET,
        "the solved board must carry the target invariant"
    );

    // Every move mask, enumerated from the geometry rather than from a board's legal moves -
    // no single board offers all of them (a full board has no holes to jump into at all), and
    // the masks are what the argument is about, not the positions they happen to be legal from.
    let full = Board::full().0;
    let on_board = |row: usize, col: usize| row < 7 && col < 7 && full >> (row * 8 + col) & 1 == 1;
    let mut masks = 0usize;
    for row in 0..7 {
        for col in 0..7 {
            for (dr, dc) in [(0usize, 1usize), (1, 0)] {
                let cells = [
                    (row, col),
                    (row + dr, col + dc),
                    (row + 2 * dr, col + 2 * dc),
                ];
                if !cells.iter().all(|&(r, c)| on_board(r, c)) {
                    continue;
                }
                let mask = cells.iter().fold(0u64, |m, &(r, c)| m | 1 << (r * 8 + c));
                assert_eq!(
                    Board::invariant_state(Board(mask).to_compressed_repr()),
                    0,
                    "move mask {mask:#x} changes the invariant"
                );
                masks += 1;
            }
        }
    }
    assert_eq!(masks, 38, "the English cross has 38 three-in-a-row triples");
}

/// Pins the algebraic identity `Board::normalize_after_move` rests on:
/// `g(board ^ mask) == g(board) ^ g(mask)` for every symmetry `g`, because the
/// symmetry transforms are GF(2)-linear and a move is an XOR.
///
/// Checked per-symmetry rather than only on the resulting minimum, so that a
/// `SYM_DIR_LUT` whose eight entries are permuted relative to `symmetries()`
/// fails right here instead of surfacing as a wrong feasible-set count much
/// later. Every geometrically valid `(idx, dir)` is covered - that is exactly
/// the domain the table has entries for - against both real and arbitrary
/// boards, since the identity is about bit patterns and does not care whether
/// the move is legal on the board it is applied to.
#[test]
fn test_normalize_after_move_matches_direct_normalize() {
    let full = Board::full().0;
    let samples = [
        Board::empty(),
        Board::full(),
        Board::default(),
        Board::solved(),
    ]
    .into_iter()
    .chain((0..2_000).map(|_| Board(rand::random::<u64>() & full)));
    let mut checked = 0usize;
    for board in samples {
        let syms = board.symmetries();
        for dir in Dir::enumerate() {
            for idx in board.movable_positions(dir) {
                let moved = board.toggle_mov_idx_unchecked(idx, dir);
                let direct = moved.symmetries();
                let masks = &Board::SYM_DIR_LUT[dir.index()][idx].0;
                for g in 0..8 {
                    assert_eq!(
                        direct[g],
                        syms[g] ^ masks[g],
                        "symmetry {g} of {board:?} ^ mask({idx}, {dir}) mismatched: \
                         SYM_DIR_LUT ordering disagrees with symmetries()"
                    );
                }
                assert_eq!(
                    Board::normalize_after_move(&syms, idx, dir),
                    moved.normalize(),
                    "normalize_after_move disagrees for {board:?} ({idx}, {dir})"
                );
                checked += 1;
            }
        }
    }
    // ~76 valid moves per board over 2004 boards
    assert!(checked > 100_000, "only checked {checked} move/board pairs");
}

#[test]
fn test_decompress_matches_reference() {
    // Same shape as `test_compressed_repr_matches_portable`, for the other
    // direction: pin the shift/mask chain against an independent reference that
    // scatters the in-play cells one at a time, and check it really does undo
    // compression - the property `keyset.rs` relies on when it unranks a key.
    fn reference(compressed: u64) -> u64 {
        let mut out = 0u64;
        let mut in_bit = 0;
        for bit in 0..64 {
            if Board::full().0 >> bit & 1 == 1 {
                out |= (compressed >> in_bit & 1) << bit;
                in_bit += 1;
            }
        }
        out
    }
    let samples = [0u64, (1 << Board::SLOTS) - 1, 1, 0xAAAA_AAAA]
        .into_iter()
        .chain((0..100_000).map(|_| rand::random::<u64>() & ((1 << Board::SLOTS) - 1)));
    for c in samples {
        assert_eq!(
            Board::from_compressed_repr(c).0,
            reference(c),
            "from_compressed_repr mismatch for {c:#x}"
        );
        // and it must undo compression exactly, which is the property the solver
        // actually depends on when it unranks a key back into a board
        let board = Board::from_compressed_repr(c);
        assert_eq!(
            board.to_compressed_repr(),
            c,
            "round trip failed for {c:#x}"
        );
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

    /// GF(4) weight of each *compressed-key* bit, for the move invariant.
    ///
    /// Put GF(4) = {0, 1, w, w^2} with `1 + w + w^2 = 0` and weight cell `(row, col)` by
    /// `w^(row+col)`. A move touches three consecutive collinear cells, so their exponents
    /// are consecutive and their weights sum to `w^k (1 + w + w^2) = 0` - and in
    /// characteristic 2 clearing a peg and filling a hole are the same operation, so
    /// `XOR of w^(row+col) over the pegs` is *invariant* under every move. Likewise for
    /// `w^(row-col)`. Each is two bits, packed here as `row+col` in bits 0-1 and `row-col`
    /// in bits 2-3, so one XOR carries both.
    ///
    /// Indexed by compressed-key bit rather than by raw board bit, because that is what
    /// `keyset.rs` ranks. The two orders agree: `to_compressed_repr` gathers in increasing
    /// raw bit order, which is row-major over the cross.
    ///
    /// `examples/hamming_neighbors.rs` derives this and proves what `keyset.rs` relies on:
    /// all 38 move masks evaluate to zero and all 8 symmetries map the resulting affine
    /// subspace onto itself, so every board reachable by moves and normalization - which is
    /// every board the solver ever stores - carries [`Self::INVARIANT_TARGET`].
    pub(crate) const INVARIANT_WEIGHTS: [u8; Self::SLOTS] = Self::invariant_weights();

    /// The value [`Self::invariant_state`] takes on every board the solver can reach.
    ///
    /// This is the solved board's - a single peg at the centre, compressed bit 16 - and the
    /// start board's is the same, which is the only reason normalization is safe here: a
    /// reflection maps `row+col` to `row-col` and so *swaps* the two invariants rather than
    /// fixing them, and the swap is harmless precisely because both halves are equal.
    /// `test_invariant_target_matches_start_and_solved` pins that.
    pub(crate) const INVARIANT_TARGET: u8 = Self::INVARIANT_WEIGHTS[16];

    const fn invariant_weights() -> [u8; Self::SLOTS] {
        // w^0 = 1, w^1 = w, w^2 = w + 1, as pairs of GF(2) coefficients
        const POWERS: [u8; 3] = [0b01, 0b10, 0b11];
        let mut weights = [0u8; Self::SLOTS];
        let mut index = 0usize;
        let mut row = 0usize;
        while row < 7 {
            // the cross's four short rows carry only the middle three columns
            let (first, last) = match row {
                0 | 1 | 5 | 6 => (2usize, 4usize),
                _ => (0usize, 6usize),
            };
            let mut col = first;
            while col <= last {
                // `+ 6` instead of a signed subtraction: 6 is a multiple of 3 so it leaves
                // the residue alone, and it keeps this in `usize` for `const` evaluation
                weights[index] = POWERS[(row + col) % 3] | (POWERS[(row + 6 - col) % 3] << 2);
                index += 1;
                col += 1;
            }
            row += 1;
        }
        assert!(
            index == Self::SLOTS,
            "the cross must have exactly SLOTS cells"
        );
        weights
    }

    /// Both GF(4) invariants of a compressed key, packed as in [`Self::INVARIANT_WEIGHTS`].
    ///
    /// Accepts a partial key too - `keyset.rs` takes the state of a key's high half alone by
    /// passing `high << LOW_BITS` - because the weights are indexed absolutely and XOR over
    /// a subset of the bits is just the state of that subset.
    pub(crate) fn invariant_state(key: u64) -> u8 {
        let mut state = 0u8;
        let mut rest = key;
        while rest != 0 {
            state ^= Self::INVARIANT_WEIGHTS[rest.trailing_zeros() as usize];
            rest &= rest - 1;
        }
        state
    }

    /// Gathers the 33 in-play cells of the 8x8 board representation down into the
    /// low 33 bits, in board order.
    ///
    /// Selected on `target_feature`, not on `target_arch`: `_pext_u64` is a BMI2
    /// instruction, and this used to be gated on `x86_64` alone, so a default
    /// `x86_64` build - which targets the baseline ISA, and BMI2 is not in it -
    /// emitted `pext` unconditionally and died with SIGILL on any pre-Haswell CPU.
    /// Now the fast path compiles only where BMI2 is actually enabled, and every
    /// other target gets the portable version below. Build with
    /// `-C target-cpu=x86-64-v3` (see `.cargo/config.toml`) to get the fast one.
    #[cfg(all(target_arch = "x86_64", target_feature = "bmi2"))]
    pub fn to_compressed_repr(&self) -> u64 {
        // SAFETY: this arm only exists when the `bmi2` target feature is enabled,
        // which is exactly `_pext_u64`'s requirement.
        unsafe { core::arch::x86_64::_pext_u64(self.0, Self::full().0) }
    }

    /// Portable equivalent of the BMI2 path above; see its documentation.
    /// `test_compressed_repr_matches_portable` checks the two agree.
    #[cfg(not(all(target_arch = "x86_64", target_feature = "bmi2")))]
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

    /// Scatters a compressed key back to board layout - the exact inverse of
    /// [`Self::to_compressed_repr`].
    ///
    /// Deliberately *not* the `pdep` mirror of that function's `pext`, which is the
    /// obvious thing to reach for and was tried: one instruction against the seven
    /// mask-shift pairs below, on a CPU (Zen 4) where `pdep` is the fast 3-cycle
    /// kind rather than the microcoded Zen 1/2 kind. It measured consistently
    /// *slower* - paired medians +2.01 ms over 21 reps and +1.33 ms over 31, faster
    /// in 7/21 and 8/31, with minima and p25 agreeing - so it was reverted.
    ///
    /// The likely reason is that this runs inside `keyset.rs`'s drain, where its
    /// result feeds straight into `pagoda`, which immediately picks the board apart
    /// again one byte at a time. The mask-and-shift form leaves the bit positions
    /// visible to the optimizer, which can fold that consumer into it and spread the
    /// work over many ports; `pdep` returns an opaque value on a single port and
    /// sits on the dependency chain instead. So the instruction count is not what
    /// this loop is paying for.
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

    pub fn normalize(self) -> Self {
        let mut symmetries = self.symmetries().into_iter();
        let mut min = symmetries.next().unwrap();
        for b in symmetries {
            if b < min {
                min = b;
            }
        }
        min
    }

    /// `(board ^ direction_mask(idx, dir)).normalize()`, given `syms = board.symmetries()`.
    ///
    /// A move is exactly an XOR with a constant mask (see
    /// [`Self::toggle_mov_idx_unchecked`]), and every operation the eight symmetry
    /// transforms are built from - `transpose`'s butterflies,
    /// `reverse_bits_in_bytes`, `swap_bytes`, shifts, AND-with-a-constant - is
    /// GF(2)-linear. So for every `g` in the symmetry group
    ///
    /// ```text
    /// g(board ^ mask) == g(board) ^ g(mask)
    /// ```
    ///
    /// and `g(mask)` depends only on the move, not the board: it is
    /// [`Self::SYM_DIR_LUT`], computed at compile time. That lets the caller run
    /// the transforms once per *board* and get every successor's eight symmetries
    /// for eight XORs against one cache line. Boards average ~10 moves in the
    /// rounds that matter, so the transform work is amortized about tenfold
    /// against calling [`Self::normalize`] on each result.
    ///
    /// `syms` must come from `board.symmetries()` for the same `board` the move is
    /// applied to, and `(idx, dir)` must be a geometrically valid move - i.e. one
    /// yielded by `mov_pattern_mask`/`rev_mov_pattern_mask`, which is exactly the
    /// set `SYM_DIR_LUT` has entries for.
    #[inline]
    pub fn normalize_after_move(syms: &[Self; 8], idx: usize, dir: Dir) -> Self {
        let masks = &Self::SYM_DIR_LUT[dir.index()][idx].0;
        let mut min = u64::MAX;
        for g in 0..8 {
            let candidate = syms[g].0 ^ masks[g].0;
            if candidate < min {
                min = candidate;
            }
        }
        Self(min)
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

    pub const fn count_pegs(&self) -> usize {
        self.0.count_ones() as usize
    }

    #[inline(always)]
    pub fn is_solved(&self) -> bool {
        *self == Self::solved()
    }

    /// the game is not solvable, if none of the marked fields contain a ball:
    ///
    ///  ```
    ///  //       .  .  .
    ///  //       .  x  .
    ///  // .  .  .  .  .  .  .
    ///  // .  x  .  x  .  x  .
    ///  // .  .  .  .  .  .  .
    ///  //       .  x  .
    ///  //       .  .  .
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

    const fn gen_sym_dir_lut() -> [[SymMasks; 64]; 4] {
        let mut lut = [[SymMasks([Board(0); 8]); 64]; 4];
        let mut d = 0;
        while d < 4 {
            let dir = Dir::from_index(d);
            // `direction_mask` computes `idx - 2` (West) and `idx - 2 * REPR`
            // (North), which underflow for low `idx`. That is a hard error in
            // const evaluation - the pre-existing `gen_luts` only gets away with
            // it because nothing references its output, so it is never evaluated.
            // Restricting to the positions a move in `dir` can actually start
            // from avoids the underflow and leaves the unreachable entries zero.
            let movable = Board::empty().movable_positions(dir);
            let mut i = 0;
            while i < 64 {
                if (movable.0 >> i) & 1 == 1 {
                    lut[d][i] = SymMasks(Self::direction_mask(i, dir).symmetries());
                }
                i += 1;
            }
            d += 1;
        }
        lut
    }

    /// `SYM_DIR_LUT[dir][idx][g]` is the `g`th symmetry of `direction_mask(idx, dir)`,
    /// in the same order [`Self::symmetries`] returns - guaranteed by construction,
    /// since the table is literally `symmetries()` applied to each move mask.
    ///
    /// Indexed `[dir][idx]` so a move's eight masks are one contiguous, 64-byte
    /// aligned block: a single cache line. 16 KiB total, of which only the ~76
    /// geometrically valid moves are ever touched.
    pub(crate) const SYM_DIR_LUT: [[SymMasks; 64]; 4] = Self::gen_sym_dir_lut();

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

    pub fn get_legal_moves(self) -> Vec<Move> {
        let mut legal_moves = Vec::new();
        for idx in self {
            let y = idx as Idx / Board::REPR;
            let x = idx as Idx % Board::REPR;
            for dir in Dir::enumerate() {
                if let Some(mov) = self.get_legal_move((y, x), dir) {
                    legal_moves.push(mov);
                }
            }
        }
        legal_moves
    }

    pub fn get_legal_inverse_moves(self) -> Vec<Move> {
        let mut legal_moves = Vec::new();
        for idx in self {
            let y = idx as Idx / Board::REPR;
            let x = idx as Idx % Board::REPR;
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
            if !self.occupied(pos) {
                return None;
            }
            self.get_legal_move(pos, dir)
        } else {
            None
        }
    }

    /// reverses the bit order *within* each byte, leaving byte positions untouched.
    ///
    /// `u64::reverse_bits` reverses byte order (like `swap_bytes`) *and* bit order
    /// within each byte; those two permutations are independent and commute, so
    /// `reverse_bits(x) == swap_bytes(reverse_bits_in_bytes(x))`. Reversing bits
    /// only within bytes needs 3 SWAR stages instead of the 6 a full 64-bit
    /// `reverse_bits` needs, so callers that also want a byte-order change (or
    /// none at all) can get it almost for free via `swap_bytes` instead of paying
    /// for a second full bit-reversal.
    #[inline]
    const fn reverse_bits_in_bytes(x: u64) -> u64 {
        let x = ((x & 0xAAAAAAAAAAAAAAAA) >> 1) | ((x & 0x5555555555555555) << 1);
        let x = ((x & 0xCCCCCCCCCCCCCCCC) >> 2) | ((x & 0x3333333333333333) << 2);
        ((x & 0xF0F0F0F0F0F0F0F0) >> 4) | ((x & 0x0F0F0F0F0F0F0F0F) << 4)
    }

    #[inline]
    pub const fn reverse_rows(&self) -> Self {
        // swap_bytes(x).reverse_bits() == reverse_bits_in_bytes(x) (the two swap_bytes cancel)
        Self(Self::reverse_bits_in_bytes(self.0) >> 1)
    }

    #[inline]
    pub const fn reverse_cols(&self) -> Self {
        Self(self.0.swap_bytes() >> 8)
    }

    #[inline]
    pub const fn rotate_180(&self) -> Self {
        Self(Self::reverse_bits_in_bytes(self.0).swap_bytes() >> 9)
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
        let rotate_270 = transposed.reverse_cols();

        // reverse_rows/rotate_180 (and their transposed counterparts rotate_90/
        // anti_transpose) both derive from `reverse_bits_in_bytes`; computing it
        // once per base value instead of once per method call avoids doing the
        // same 3-stage SWAR pass twice.
        let pbr_self = Self::reverse_bits_in_bytes(self.0);
        let reverse_rows = Self(pbr_self >> 1);
        let rotate_180 = Self(pbr_self.swap_bytes() >> 9);

        let pbr_transposed = Self::reverse_bits_in_bytes(transposed.0);
        let rotate_90 = Self(pbr_transposed >> 1);
        let anti_transpose = Self(pbr_transposed.swap_bytes() >> 9);

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

    pub fn possible_moves(states: &[Self]) -> Vec<Self> {
        // count first so the vec is allocated exactly once, at its final size;
        // pushing without this reserve was the single largest source of peak
        // memory use (repeated grow+copy from doubling, plus leftover slack
        // from over-allocation), since this runs on every generated board.
        let total: usize = Dir::enumerate()
            .into_iter()
            .map(|dir| {
                states
                    .iter()
                    .map(|board| board.mov_pattern_mask(dir).count_pegs())
                    .sum::<usize>()
            })
            .sum();
        let mut constellations = Vec::with_capacity(total);
        for dir in Dir::enumerate() {
            for board in states {
                for idx in board.mov_pattern_mask(dir) {
                    constellations.push(board.toggle_mov_idx_unchecked(idx, dir));
                }
            }
        }
        constellations
    }

    pub fn possible_reverse_moves(states: &[Self]) -> Vec<Self> {
        let total: usize = Dir::enumerate()
            .into_iter()
            .map(|dir| {
                states
                    .iter()
                    .map(|board| board.rev_mov_pattern_mask(dir).count_pegs())
                    .sum::<usize>()
            })
            .sum();
        let mut constellations = Vec::with_capacity(total);
        for dir in Dir::enumerate() {
            for board in states {
                for idx in board.rev_mov_pattern_mask(dir) {
                    constellations.push(board.toggle_mov_idx_unchecked(idx, dir));
                }
            }
        }
        constellations
    }

    pub fn normalize_all(constellations: &mut [Self]) {
        for board in constellations {
            *board = board.normalize();
        }
    }

    ///
    /// ```rust
    /// use solitaire_solver::Board;
    /// let and = Board::type_masks().into_iter().reduce(std::ops::BitAnd::bitand).unwrap();
    /// let or = Board::type_masks().into_iter().reduce(std::ops::BitOr::bitor).unwrap();
    /// assert_eq!(and, Board::empty());
    /// assert_eq!(or, Board::full());
    /// ```
    pub fn type_masks() -> [Self; 4] {
        [
            ". . .
             . o .
         . . . . . . .
         . o . o . o .
         . . . . . . .
             . o .
             . . .
        "
            .try_into()
            .unwrap(),
            "o . o
             . . .
         o . o . o . o
         . . . . . . .
         o . o . o . o
             . . .
             o . o
        "
            .try_into()
            .unwrap(),
            ". . .
             o . o
         . . . . . . .
         o . o . o . o
         . . . . . . .
             o . o
             . . .
        "
            .try_into()
            .unwrap(),
            ". o .
             . . .
         . o . o . o .
         . . . . . . .
         . o . o . o .
             . . .
             . o .
        "
            .try_into()
            .unwrap(),
        ]
    }
}

impl IntoIterator for Board {
    type Item = usize;

    type IntoIter = PegIter;

    fn into_iter(self) -> Self::IntoIter {
        PegIter(self)
    }
}

impl TryFrom<&'_ str> for Board {
    type Error = &'static str;

    fn try_from(s: &'_ str) -> Result<Self, Self::Error> {
        let lines = s.lines();
        let mut board = Board::empty();
        for (y, l) in lines.enumerate() {
            let mut x = 0;
            for c in l.chars() {
                match c {
                    'o' => {
                        {
                            let x = match y {
                                0 | 1 | 5 | 6 => x + 2,
                                _ => x,
                            };
                            board = board.set((y as Idx, x as Idx));
                        }
                        x += 1;
                    }
                    '.' => {
                        x += 1;
                    }
                    ' ' => {}
                    _ => return Err("invalid character"),
                }
            }
        }
        Ok(board)
    }
}

#[test]
fn test_parse() {
    let full = Board::try_from(
        "o o o
         o o o
    o o o o o o o
    o o o o o o o
    o o o o o o o
        o o o
        o o o
    ",
    )
    .unwrap();
    eprintln!("{full}");
    assert_eq!(full, Board::full());
}

pub struct PegIter(Board); // or whatever your inner type is

impl Iterator for PegIter {
    type Item = usize;
    fn next(&mut self) -> Option<Self::Item> {
        if self.0 == Board::empty() {
            return None;
        }
        let idx = self.0.0.trailing_zeros() as Self::Item;
        self.0.0 &= self.0.0 - 1;
        Some(idx)
    }
}
