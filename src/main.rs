use std::{collections::HashSet, num::NonZero};

/// Counts every allocation the run makes, behind the `count-allocs` feature.
///
/// Exists because the obvious tools do not work here: `dhat` and `massif` both need
/// valgrind, and valgrind cannot decode the GFNI symmetry primitives
/// (`VGF2P8AFFINEQB`) this builds with, so it dies with SIGILL before reaching the
/// solver. This counts in-process at full speed instead, which is enough to answer
/// "how many allocations, how many bytes, and how big" - the questions that actually
/// bear on whether allocation is worth optimizing.
///
/// What it found, for the record: `calculate-all` makes ~1_791 allocations and churns
/// ~172 MB per solve, so the count is negligible and the bytes are what matter. The
/// bytes are ~13 large blocks - the 8.7 MiB keyset bitmap, the retained per-layer
/// `Vec<Board>`s that are the answer itself, and two exactly-sized tail buffers of
/// 6.4 and 12.8 MiB. That is why swapping the allocator moved so little.
///
/// Enable with `cargo run --release --features count-allocs -- --repeat 1 calculate-all`.
/// Differencing two repeat counts separates per-solve cost from startup.
#[cfg(feature = "count-allocs")]
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    pub static ALLOCS: AtomicU64 = AtomicU64::new(0);
    pub static BYTES: AtomicU64 = AtomicU64::new(0);
    pub static REALLOCS: AtomicU64 = AtomicU64::new(0);
    pub static REALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
    /// allocations by power-of-two size class
    pub static BUCKETS: [AtomicU64; 48] = [const { AtomicU64::new(0) }; 48];

    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size() as u64, Relaxed);
            let class = (64 - (layout.size() as u64 | 1).leading_zeros()).min(47) as usize;
            BUCKETS[class].fetch_add(1, Relaxed);
            log_big(layout.size());
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            REALLOCS.fetch_add(1, Relaxed);
            REALLOC_BYTES.fetch_add(new_size as u64, Relaxed);
            unsafe { System.realloc(ptr, layout, new_size) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size() as u64, Relaxed);
            let class = (64 - (layout.size() as u64 | 1).leading_zeros()).min(47) as usize;
            BUCKETS[class].fetch_add(1, Relaxed);
            log_big(layout.size());
            unsafe { System.alloc_zeroed(layout) }
        }
    }

    /// Logs every allocation of 2 MiB or more, in order, so the big ones can be matched
    /// against the algorithm's own logged layer sizes. Deliberately not a backtrace: taking
    /// one inside a global allocator reenters the allocator.
    fn log_big(size: usize) {
        if size >= 2 << 20 {
            let n = BIG.fetch_add(1, Relaxed);
            eprintln!("BIG #{n} {size} bytes ({:.1} MiB)", size as f64 / 1048576.0);
        }
    }

    pub static BIG: AtomicU64 = AtomicU64::new(0);

    pub fn report() {
        use std::sync::atomic::Ordering::Relaxed;
        eprintln!(
            "ALLOC allocs={} bytes={} reallocs={} realloc_bytes={}",
            ALLOCS.load(Relaxed),
            BYTES.load(Relaxed),
            REALLOCS.load(Relaxed),
            REALLOC_BYTES.load(Relaxed),
        );
        for (class, count) in BUCKETS.iter().enumerate() {
            let n = count.load(Relaxed);
            if n > 0 {
                eprintln!("ALLOC   <2^{class:<2} {n:>10}");
            }
        }
    }
}

#[cfg(feature = "count-allocs")]
#[global_allocator]
static COUNTING: counting::Counting = counting::Counting;

use clap::{Parser, Subcommand};
use solitaire_solver::Board;

#[derive(Parser)]
struct Args {
    /// print the solution
    #[arg(short, long)]
    print: bool,
    /// number of threads to use for all solutions
    #[arg(short, long)]
    threads: Option<NonZero<usize>>,
    /// repeat `calculate-all` this many times in one process
    ///
    /// For profiling and benchmarking a computation that takes ~100ms: sampling one
    /// run yields few samples, and repeating the *process* instead charges every
    /// iteration for startup and first touch of the maps - which together are a
    /// couple of percent of a run, all of it noise relative to the loops one is
    /// usually trying to measure. Repeating in-process
    /// keeps the allocator and page tables warm, so iterations after the first
    /// measure steady state.
    ///
    /// Each iteration logs its own internal timing at `RUST_LOG=info`. Discard the
    /// first *two*: measured over repeated 10-12 iteration runs, they come in around
    /// 111/134 and 101/120 ms against a steady state of 93-99, so the warm-up is
    /// two iterations rather than one. Steady state does come out ~2-3% under
    /// separate processes of the same binary (93-99 vs 98-105 ms), which is the
    /// startup and teardown this exists to stop paying.
    #[arg(short, long, default_value_t = 1)]
    repeat: usize,
    /// subcommands
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Clone, Debug, PartialEq, Eq)]
enum Command {
    /// calculate all solutions
    CalculateAll,
    /// calculate all solutions (naive recursively)
    CalculateAllNaive,
    /// calculate a single solution
    CalculateSingle,
    /// compare naive and advanced solution (sanity check)
    CompareSolutions,
    /// calculate success ratio when chosing moves at random
    CalculateRandomChanceSuccessRatio,
    /// calculate unique solutions
    UniqueSolutions,
    /// calculate unique paths of solutions
    UniquePaths,
    /// count solutions distinct by which peg jumped which, tracking pegs by start slot
    UniqueJumpMaps,
}

fn main() {
    let args = Args::parse();
    #[cfg(not(feature = "game"))]
    {
        use env_logger::Env;

        let env = Env::default().filter_or("RUST_LOG", "info");
        env_logger::init_from_env(env);
    }
    match args.command {
        Some(command) => {
            #[cfg(feature = "game")]
            {
                use env_logger::Env;

                let env = Env::default().filter_or("RUST_LOG", "info");
                env_logger::init_from_env(env);
            }
            match command {
                Command::CalculateAll => {
                    // `black_box` so nothing about the repetition lets the optimizer
                    // conclude that later iterations are redundant, and so the result
                    // has to be materialized rather than folded into a length
                    let mut solutions = 0;
                    for _ in 0..args.repeat.max(1) {
                        let vec = solitaire_solver::calculate_feasible_set(args.threads);
                        solutions = std::hint::black_box(&vec).len();
                    }
                    println!("solutions: {solutions}");
                }
                Command::CalculateAllNaive => {
                    solitaire_solver::calculate_all_solutions_naive();
                }
                Command::CalculateRandomChanceSuccessRatio => {
                    let feasible = solitaire_solver::calculate_feasible_set(args.threads);
                    let start = std::time::Instant::now();
                    let feasible: Vec<_> = feasible.into_iter().collect();
                    let success_probabilities =
                        solitaire_solver::calculate_p_random_chance_success(feasible.into_iter());
                    let p = *success_probabilities.get(&Board::default()).unwrap();
                    let percentage = p * 100.;

                    println!("took {:?}", start.elapsed());
                    println!("success probability when chosing moves at random: {percentage}%");
                    let (b, p) = success_probabilities
                        .iter()
                        .map(|(b, p)| (*b, *p))
                        .fold((Board::default(), f64::INFINITY), |(b1, p1), (b2, p2)| {
                            if p2 < p1 { (b2, p2) } else { (b1, p1) }
                        });
                    let perc = p * 100.;
                    println!("minimum success chance: \n{b} ({perc}%)");
                }
                Command::CalculateSingle => {
                    let solution = solitaire_solver::calculate_first_solution();
                    if args.print {
                        solitaire_solver::print_solution(solution);
                    }
                }
                Command::CompareSolutions => {
                    let solutions: HashSet<Board> = solitaire_solver::calculate_feasible_set(None)
                        .into_iter()
                        .collect();
                    let solutions_naive: HashSet<Board> =
                        solitaire_solver::calculate_all_solutions_naive()
                            .into_iter()
                            .collect();
                    assert_eq!(solutions, solutions_naive)
                }
                Command::UniqueJumpMaps => {
                    let feasible = solitaire_solver::calculate_feasible_set(args.threads);
                    log::info!("feasible: {}", feasible.len());
                    let maps =
                        solitaire_solver::all_unique_jump_maps(Board::default(), feasible);
                    log::info!("unique jump maps: {}", maps.len());
                }
                Command::UniqueSolutions => {
                    let feasible = solitaire_solver::calculate_feasible_set(args.threads);
                    log::info!("feasible: {}", feasible.len());
                    let solutions =
                        solitaire_solver::all_unique_solutions(Board::default(), feasible);
                    log::info!("unique solutions: {}", solutions.len());
                }
                Command::UniquePaths => {
                    let feasible = solitaire_solver::calculate_feasible_set(args.threads);
                    log::info!("feasible: {}", feasible.len());
                    let moves = solitaire_solver::all_unique_paths(feasible.clone(), args.threads);
                    log::info!(
                        "distinct move sequences:  {}",
                        moves.get(&Board::default()).unwrap()
                    );
                    let boards = solitaire_solver::all_unique_board_paths(feasible, args.threads);
                    log::info!(
                        "distinct board sequences: {}",
                        boards.get(&Board::default()).unwrap()
                    );
                }
            }
        }
        None => {
            #[cfg(feature = "game")]
            peg_solitaire::run();

            #[cfg(not(feature = "game"))]
            {
                eprintln!("\"game\" feature not enabled!");
                std::process::exit(1)
            }
        }
    }
    #[cfg(feature = "count-allocs")]
    counting::report();
}
