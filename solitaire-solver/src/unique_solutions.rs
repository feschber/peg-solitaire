use crate::board::Idx;
use crate::dir::Dir;
use crate::par;
use crate::solution::SolutionMultiset;
use crate::{Board, Move};
use crate::{HashMap, HashSet, Solution};
use rustc_hash::FxHashSet;
use std::collections::BTreeMap;
use std::num::NonZero;

/// Dense slot for a move, keyed by its starting bit and direction: 64 positions x 4
/// directions. Sparse - only 76 of the 256 are reachable - but it makes the occurrence
/// counter a flat array indexed by arithmetic rather than a map.
const MOVE_SLOTS: usize = 64 * 4;

/// A solution is 31 moves, so no single move can occur more often than that.
const MAX_OCCURRENCES: usize = 32;

fn move_slot(idx: usize, dir: Dir) -> usize {
    idx * 4 + dir.index()
}

/// The `Move` a starting bit and direction describe.
///
/// Pure geometry, so it needs no board: `skip` is one step along `dir` and `target` two.
/// `test_move_at_matches_get_legal_moves` pins it against `Board::get_legal_moves`, which
/// derives the same thing the long way round.
fn move_at(idx: usize, dir: Dir) -> Move {
    let row = (idx / Board::REPR as usize) as i32;
    let col = (idx % Board::REPR as usize) as i32;
    let (d_row, d_col) = match dir {
        Dir::North => (-1, 0),
        Dir::South => (1, 0),
        Dir::West => (0, -1),
        Dir::East => (0, 1),
    };
    let step = |k: i32| (((row + d_row * k) as Idx), ((col + d_col * k) as Idx));
    Move {
        pos: (row as Idx, col as Idx),
        skip: step(1),
        target: step(2),
    }
}

/// Random value per (move slot, occurrence count), for hashing a move *multiset*
/// incrementally.
///
/// Deterministic rather than `rand::random`: the table is a pure function of the seed, so a
/// run is reproducible, and it is built once up front instead of being filled lazily through
/// a `HashMap` lookup on every edge.
///
/// `count == 0` is deliberately zero, which makes the hash of a multiset exactly the XOR of
/// `table[slot][count]` over the slots present - a move at count zero contributes nothing.
struct Zobrist {
    table: Vec<[u64; MAX_OCCURRENCES]>,
}

impl Zobrist {
    fn new() -> Self {
        // splitmix64, so each entry is an independent-looking constant with no state to carry
        const fn mix(mut x: u64) -> u64 {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            x ^ (x >> 31)
        }
        Self {
            table: (0..MOVE_SLOTS)
                .map(|slot| {
                    let mut row = [0u64; MAX_OCCURRENCES];
                    for (count, value) in row.iter_mut().enumerate().skip(1) {
                        *value = mix(((slot as u64) << 8) | count as u64);
                    }
                    row
                })
                .collect(),
        }
    }

    /// XOR to apply when a move's count goes from `count - 1` to `count`, or back.
    fn delta(&self, slot: usize, count: usize) -> u64 {
        self.table[slot][count - 1] ^ self.table[slot][count]
    }
}

type MultisetHash = u64;

/// Depth-first search over the feasible graph, collecting the distinct move multisets that
/// reach the solved board.
///
/// The state is maintained *in place* and undone on the way back out, which is the whole
/// point: the previous version pushed `(board, multiset, hash)` onto an explicit stack and
/// so cloned a `BTreeMap` for every edge it pushed - around 85 million of them for the
/// central game. Here the multiset is one flat counter array, a move costs an increment and
/// an XOR, and a `BTreeMap` is built only when a solution is actually found (12_752 times).
struct Search<'a> {
    feasible: &'a HashSet<Board>,
    zobrist: Zobrist,
    /// occurrences of each move slot along the current path
    counts: Vec<u8>,
    /// multiset hashes already expanded - see `visit` for why the board is not part of the key
    visited: FxHashSet<MultisetHash>,
    solutions: FxHashSet<SolutionMultiset>,
}

impl Search<'_> {
    /// Expands `board`, whose path so far hashes to `hash`.
    ///
    /// `visited` keys on the multiset hash *alone*, where the previous version used
    /// `(board, hash)`. That is not a weakening: a move is an XOR with a fixed mask and XOR
    /// commutes, so the board is `start ^ (XOR of the masks applied, with even multiplicities
    /// cancelling)` - i.e. the multiset determines the board. Adding the board to the key
    /// therefore partitions nothing further, and dropping it halves the table, which at ~85M
    /// entries was most of the 3.4 GB this used to need.
    fn visit(&mut self, board: Board, hash: MultisetHash) {
        if board.is_solved() {
            self.solutions.insert(self.materialize());
            return;
        }

        // `symmetries` once per board, then `normalize_after_move` XORs each move's mask into
        // the eight images - the identity `g(b ^ m) = g(b) ^ g(m)`. The previous version
        // called `board.mov(mov).normalize()`, normalizing every successor from scratch: eight
        // full symmetry transforms per edge instead of eight XORs.
        let syms = board.symmetries();
        for dir in Dir::enumerate() {
            for idx in board.mov_pattern_mask(dir) {
                if !self
                    .feasible
                    .contains(&Board::normalize_after_move(&syms, idx, dir))
                {
                    continue;
                }
                let slot = move_slot(idx, dir);
                self.counts[slot] += 1;
                let next_hash = hash ^ self.zobrist.delta(slot, self.counts[slot] as usize);
                if self.visited.insert(next_hash) {
                    // the search itself walks un-normalized boards, as it did before; only the
                    // feasibility test above is symmetry-reduced
                    self.visit(board.toggle_mov_idx_unchecked(idx, dir), next_hash);
                }
                self.counts[slot] -= 1;
            }
        }
    }

    /// Turns the current counter array into the `BTreeMap` the API returns. Only ever called
    /// on reaching a solution, so it can afford to be the slow part.
    fn materialize(&self) -> SolutionMultiset {
        let mut multiset = BTreeMap::new();
        for (slot, &count) in self.counts.iter().enumerate() {
            if count > 0 {
                multiset.insert(move_at(slot / 4, Dir::from_index(slot % 4)), count as usize);
            }
        }
        multiset
    }
}

/// we define two solutions as "equal" when the
///  multiset of steps is equivalent between them
///
/// Finds all *unique* solutions (by step-multiset) from `start` to any board in `goals`.
///
/// For the central game this is 12_752 multisets, against 40_861_647_040_079_968 move
/// sequences - so each multiset admits on the order of 10^12 valid orderings, which is why
/// enumerating the classes is tractable at all while enumerating the sequences is not.
pub fn all_unique_solutions(
    start: Board,
    feasible: impl IntoIterator<Item = Board>,
) -> FxHashSet<SolutionMultiset> {
    log::info!("calculating unique solutions ....");
    let feasible: HashSet<Board> = feasible.into_iter().collect();
    let mut search = Search {
        feasible: &feasible,
        zobrist: Zobrist::new(),
        counts: vec![0u8; MOVE_SLOTS],
        visited: FxHashSet::default(),
        solutions: FxHashSet::default(),
    };
    search.visited.insert(0);
    search.visit(start, 0);
    search.solutions
}

/// Sentinel in a [`JumpMap`] for the one peg that is never removed.
pub const NOT_REMOVED: u8 = u8::MAX;

/// Which peg jumped which, both identified by the slot the peg *started* in.
///
/// Indexed by the victim's starting slot; the value is the jumper's. Every peg is removed
/// exactly once - 32 pegs down to 1, one removal per move - so this is a *function* with 31
/// entries and distinct keys, not a multiset. That is what makes the whole equivalence
/// cheaper than the move-multiset one: no occurrence counting, and the canonical form is a
/// fixed-size array rather than a `BTreeMap`.
///
/// Raw bit positions, so the array is 64 wide with the off-board indices unused.
pub type JumpMap = [u8; 64];

/// Skip and target bit positions of the move starting at `idx` and going in `dir`.
fn move_bits(idx: usize, dir: Dir) -> (usize, usize) {
    let step = match dir {
        Dir::North => -(Board::REPR as isize),
        Dir::South => Board::REPR as isize,
        Dir::West => -1,
        Dir::East => 1,
    };
    let at = |k: isize| (idx as isize + step * k) as usize;
    (at(1), at(2))
}

/// Depth-first search collecting the distinct [`JumpMap`]s that reach the solved board.
///
/// Separate from [`Search`] rather than sharing it, because the two equivalences need
/// genuinely different state: the multiset one is a function of the moves alone, while this
/// one depends on *peg identity*, which is a function of the whole path.
///
/// That difference is also why this may not be tractable where the multiset version is. The
/// multiset version prunes on the multiset alone, since a move is an XOR with a fixed mask
/// and so the multiset determines the board. Here two paths can reach the same board with the
/// same partial jump map and still have their pegs arranged differently, which changes every
/// pair they can produce afterwards - so the state that determines the future is
/// `(board, identity assignment, partial map)`, and there are far more of those than boards.
struct JumpSearch<'a> {
    feasible: &'a HashSet<Board>,
    /// random value per (slot, peg identity), for hashing the identity assignment
    placed: Vec<u64>,
    /// random value per (victim, jumper), for hashing the partial map
    jumped: Vec<u64>,
    /// slot -> starting slot of the peg currently in it; only occupied slots are meaningful
    identity: JumpMap,
    /// the canonical form being built
    map: JumpMap,
    visited: FxHashSet<(Board, u64)>,
    maps: FxHashSet<JumpMap>,
    states: u64,
}

impl JumpSearch<'_> {
    fn visit(&mut self, board: Board, hash: u64) {
        if board.is_solved() {
            self.maps.insert(self.map);
            return;
        }
        self.states += 1;
        if self.states.is_multiple_of(50_000_000) {
            log::info!(
                "jump maps: {} states expanded, {} distinct maps so far",
                self.states,
                self.maps.len()
            );
        }

        let syms = board.symmetries();
        for dir in Dir::enumerate() {
            for idx in board.mov_pattern_mask(dir) {
                if !self
                    .feasible
                    .contains(&Board::normalize_after_move(&syms, idx, dir))
                {
                    continue;
                }
                let (skip, target) = move_bits(idx, dir);
                let jumper = self.identity[idx];
                let victim = self.identity[skip];

                // The hash covers the identity assignment *and* the partial map, so that two
                // states are merged only when both agree - see the note on the struct.
                let next_hash = hash
                    ^ self.placed[idx * 64 + jumper as usize]
                    ^ self.placed[skip * 64 + victim as usize]
                    ^ self.placed[target * 64 + jumper as usize]
                    ^ self.jumped[victim as usize * 64 + jumper as usize];

                let next_board = board.toggle_mov_idx_unchecked(idx, dir);
                if !self.visited.insert((next_board, next_hash)) {
                    continue;
                }

                let vacated = self.identity[target];
                self.identity[target] = jumper;
                self.map[victim as usize] = jumper;

                self.visit(next_board, next_hash);

                self.map[victim as usize] = NOT_REMOVED;
                self.identity[target] = vacated;
            }
        }
    }
}

/// Finds the distinct [`JumpMap`]s of all solutions from `start`.
///
/// Two solutions are the same here when every peg was jumped by the same peg, tracking pegs
/// by where they started - the equivalence [`all_unique_solutions`]'s move multiset does not
/// capture, since that identifies moves by *slots* and so cannot tell which physical peg made
/// the jump.
pub fn all_unique_jump_maps(
    start: Board,
    feasible: impl IntoIterator<Item = Board>,
) -> FxHashSet<JumpMap> {
    log::info!("calculating unique jump maps ....");
    let feasible: HashSet<Board> = feasible.into_iter().collect();

    const fn mix(mut x: u64) -> u64 {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    }

    let mut identity = [NOT_REMOVED; 64];
    for bit in start {
        identity[bit] = bit as u8;
    }

    let mut search = JumpSearch {
        feasible: &feasible,
        placed: (0..64 * 64).map(|i| mix(i as u64)).collect(),
        jumped: (0..64 * 64).map(|i| mix(0x5EED_0000_0000_0000 | i as u64)).collect(),
        identity,
        map: [NOT_REMOVED; 64],
        visited: FxHashSet::default(),
        maps: FxHashSet::default(),
        states: 0,
    };
    search.visit(start, 0);
    log::info!("jump maps: {} states expanded", search.states);
    search.maps
}

#[allow(unused)]
fn canonicalize(
    unique_solutions: FxHashSet<SolutionMultiset>,
    feasible: HashSet<Board>,
) -> Vec<[Board; 32]> {
    for s in &unique_solutions {
        for (s, c) in s {
            for _ in 0..*c {
                print!("{s} ");
            }
        }
        println!();
    }
    // canonicalize => sort multiset,
    // then always take first possible move on initial board.
    // Deduplicate by normalizing the boards and rehashing
    let unique_solutions: FxHashSet<Solution> = unique_solutions
        .into_iter()
        .map(|b| Solution::from((b, &feasible)))
        .collect();
    log::info!(
        "unique solutions by move multiset: {}",
        unique_solutions.len()
    );
    for s in &unique_solutions {
        println!("{s}");
    }

    let unique_solutions: FxHashSet<[Board; 32]> = unique_solutions
        .into_iter()
        .map(<[Board; 32]>::from)
        .map(|mut s| {
            s.iter_mut().for_each(|b| *b = b.normalize());
            s
        })
        .collect();
    let mut unique_solutions: Vec<_> = unique_solutions.into_iter().collect();
    unique_solutions.sort();

    unique_solutions
}

/// Upper bound on the moves available from one board.
///
/// The cross holds 38 collinear triples and each can be jumped from either end, so no
/// position can offer more than 76. Lets the successor buffer live on the stack instead of
/// being heap-allocated per board - the old code allocated a `Vec` for every one of the
/// 1_679_072 feasible boards.
const MAX_MOVES: usize = 76;

/// How to treat two different moves from the same board that reach the same normalized
/// successor - see [`all_unique_paths`] and [`all_unique_board_paths`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum PathKind {
    /// Count both. Paths are sequences of *moves*.
    MoveSequences,
    /// Count once. Paths are sequences of *normalized boards*.
    BoardSequences,
}

/// For every feasible board, the number of distinct **move sequences** taking it to the
/// solved board.
///
/// From the start board this is 40_861_647_040_079_968, the published solution count for the
/// central game, which is what pins this variant as correct.
///
/// Two different moves reaching the same normalized successor count as two paths here,
/// because they are two different sequences of moves. [`all_unique_board_paths`] is the
/// variant that collapses them.
pub fn all_unique_paths(
    feasible: impl IntoIterator<Item = Board>,
    threads: Option<NonZero<usize>>,
) -> HashMap<Board, u64> {
    count_paths(feasible, PathKind::MoveSequences, threads)
}

/// For every feasible board, the number of distinct sequences of **normalized boards**
/// taking it to the solved board.
///
/// Differs from [`all_unique_paths`] only where a board has two moves reaching the same
/// normalized successor - a board with a nontrivial stabilizer. From the start board this is
/// 4_750_671_971_732_176 against the move-sequence count's 40_861_647_040_079_968.
pub fn all_unique_board_paths(
    feasible: impl IntoIterator<Item = Board>,
    threads: Option<NonZero<usize>>,
) -> HashMap<Board, u64> {
    count_paths(feasible, PathKind::BoardSequences, threads)
}

/// Shared layered dynamic program: a board's count is the sum of its successors' counts, and
/// a move always removes exactly one peg, so layer `k` depends only on layer `k - 1`.
///
/// Three things this does differently from the obvious formulation, in decreasing order of
/// what they were worth:
///
/// - Successors come from `normalize_after_move`, not `possible_moves` + `normalize_all`. The
///   latter normalizes each successor from scratch, which is 8 symmetry transforms *per
///   successor*; the former takes the board's 8 symmetries once and XORs the move's mask into
///   each, which is the identity `g(b ^ m) = g(b) ^ g(m)` that `board.rs` already relies on.
///   Over 17.2M successors that is 137M symmetry transforms replaced by 17.2M XOR rounds.
/// - One `HashMap<Board, u32>` index instead of a `HashSet` plus a `HashMap`, so a successor
///   costs one hash lookup rather than two: finding it *is* finding where its count lives.
/// - The successor buffer is a stack array reused per board rather than a fresh `Vec`, which
///   removes one heap allocation per feasible board.
///
/// Each layer is then independent, so it is evaluated in parallel and written back in order.
fn count_paths(
    feasible: impl IntoIterator<Item = Board>,
    kind: PathKind,
    threads: Option<NonZero<usize>>,
) -> HashMap<Board, u64> {
    let mut index: HashMap<Board, u32> = HashMap::default();
    // `Board::SLOTS + 2` rather than 33: `count_pegs` reaches `SLOTS`, and the old fixed
    // `[_; 33]` would have panicked on a board with that many pegs. No feasible board has
    // (the start has one hole) but nothing here guarantees the input is the feasible set.
    let mut by_pegs: Vec<Vec<Board>> = vec![Vec::new(); Board::SLOTS + 2];
    for board in feasible {
        let next = index.len() as u32;
        index.insert(board, next);
        by_pegs[board.count_pegs()].push(board);
    }

    let mut counts = vec![0u64; index.len()];
    if let Some(&solved) = index.get(&Board::solved()) {
        counts[solved as usize] = 1;
    }

    // `None` means "as many as the machine has", matching `calculate_feasible_set`. Passing
    // the count down to `par::parallel` is what makes `--threads 1` actually sequential:
    // `par_map_chunks` short-circuits to one chunk on the calling thread at 1, whereas
    // `configure_thread_pool` cannot help - it leaves an already-built pool alone, so by this
    // point the pool's width is whatever the first caller asked for.
    let threads = threads.unwrap_or_else(par::num_threads).get();
    // skip(2): layer 0 is empty and layer 1 is the solved board, seeded above
    for layer in by_pegs.iter().skip(2) {
        if layer.is_empty() {
            continue;
        }
        // Read-only view of everything below this layer; the writes land afterwards, so no
        // two tasks touch the same count and none reads one this layer is producing.
        let (index_ref, counts_ref) = (&index, &counts);
        let layer_counts: Vec<u64> = par::parallel(layer, threads, move |chunk| {
            chunk
                .iter()
                .map(|board| {
                    let mut buffer = [Board::empty(); MAX_MOVES];
                    let successors = successors_of(*board, &mut buffer, kind);
                    successors
                        .iter()
                        .filter_map(|s| index_ref.get(s))
                        .map(|&i| counts_ref[i as usize])
                        .sum()
                })
                .collect()
        });
        for (board, count) in layer.iter().zip(layer_counts) {
            counts[index[board] as usize] = count;
        }
    }

    index
        .iter()
        .map(|(board, &i)| (*board, counts[i as usize]))
        .collect()
}

/// Fills `buffer` with `board`'s normalized successors and returns the filled prefix,
/// deduplicated when `kind` asks for it.
///
/// Deduplication sorts first, deliberately. A bare `dedup` removes only *adjacent* equals,
/// and `possible_moves` groups its output by direction, so two moves in different directions
/// reaching the same successor land far apart and survive it - 2_999 of them across the
/// feasible set, each double-counting a subtree. That is what the previous version of this
/// did, and it is why its answer matched neither of the two well-defined counts.
fn successors_of(board: Board, buffer: &mut [Board; MAX_MOVES], kind: PathKind) -> &[Board] {
    let syms = board.symmetries();
    let mut len = 0;
    for dir in Dir::enumerate() {
        for idx in board.mov_pattern_mask(dir) {
            debug_assert!(
                len < MAX_MOVES,
                "more than {MAX_MOVES} moves from one board"
            );
            buffer[len] = Board::normalize_after_move(&syms, idx, dir);
            len += 1;
        }
    }
    let filled = &mut buffer[..len];
    if kind == PathKind::BoardSequences {
        filled.sort_unstable();
        // in-place dedup: `slice::partition_dedup` would do this but is still unstable
        let mut write = 0;
        for read in 0..len {
            if write == 0 || filled[write - 1] != filled[read] {
                filled[write] = filled[read];
                write += 1;
            }
        }
        len = write;
    }
    &buffer[..len]
}

/// `move_at` derives a `Move` from a starting bit and direction by pure geometry, while
/// `Board::get_legal_moves` derives the same thing from the board via `get_legal_move`.
/// The search uses the former for every edge, so they had better agree.
#[test]
fn test_move_at_matches_get_legal_moves() {
    let boards =
        [
            Board::default(),
            Board(Board::full().0 & !Board::solved().0),
            Board::solved(),
        ]
        .into_iter()
        .chain((0..500).map(|_| {
            Board::from_compressed_repr(rand::random::<u64>() & ((1 << Board::SLOTS) - 1))
        }));

    let mut checked = 0usize;
    for board in boards {
        let mut expected = board.get_legal_moves();
        expected.sort_unstable();

        let mut derived: Vec<Move> = Dir::enumerate()
            .into_iter()
            .flat_map(|dir| {
                board
                    .mov_pattern_mask(dir)
                    .into_iter()
                    .map(move |idx| move_at(idx, dir))
            })
            .collect();
        derived.sort_unstable();

        assert_eq!(derived, expected, "move sets differ for {board:?}");
        checked += expected.len();
    }
    assert!(checked > 0, "no moves were compared");
}
