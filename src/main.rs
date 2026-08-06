use std::{collections::HashSet, num::NonZero};

use clap::{Parser, Subcommand};
use solitaire_solver::Board;

#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
struct Args {
    /// print the solution
    #[arg(short, long)]
    print: bool,
    /// number of threads to use for all solutions
    #[arg(short, long)]
    threads: Option<NonZero<usize>>,
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
}

/// Turns transparent huge pages off for this process.
///
/// mimalloc reserves its heap in 1 GiB arenas and calls `madvise(MADV_HUGEPAGE)`
/// on each. Where `transparent_hugepage/defrag` is `madvise` or `always` (the
/// former is this distro's default), that makes every fault in those arenas a
/// "try hard" huge-page allocation, which falls into *synchronous* direct
/// compaction - physically migrating pages on the fault path - whenever no free
/// 2 MiB block is around. On a fragmented machine that took an identical run from
/// 0.44s to 2.75s of system time (0.30s -> 0.66s wall) for identical userspace
/// work, with a kernel profile blaming `__do_huge_pmd_anonymous_page` and
/// `compact_zone`, while 98% of the compaction attempts failed and the process
/// still ended up with `AnonHugePages: 0 kB`.
///
/// `keyset.rs` already opts its 1 GiB bitmap out via `MADV_NOHUGEPAGE`, but that
/// is only about half the resident memory - the board vectors live in a second
/// mimalloc arena we do not control. This covers the rest.
///
/// Deliberately scoped to the solver subcommands rather than applied at startup:
/// it is a process-wide policy, and the `game` build renders through bevy, which
/// has not been evaluated against it.
#[cfg(target_os = "linux")]
fn disable_transparent_hugepages_for_process() {
    // SAFETY: prctl with PR_SET_THP_DISABLE only sets a per-process flag telling
    // the kernel not to back this process's anonymous memory with transparent huge
    // pages. It touches no memory of ours and cannot fail in a way we care about
    // (an older kernel returning EINVAL just leaves the default behaviour).
    unsafe {
        libc::prctl(libc::PR_SET_THP_DISABLE, 1, 0, 0, 0);
    }
}

#[cfg(not(target_os = "linux"))]
fn disable_transparent_hugepages_for_process() {}

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
            disable_transparent_hugepages_for_process();
            #[cfg(feature = "game")]
            {
                use env_logger::Env;

                let env = Env::default().filter_or("RUST_LOG", "info");
                env_logger::init_from_env(env);
            }
            match command {
                Command::CalculateAll => {
                    let vec = solitaire_solver::calculate_feasible_set(args.threads);
                    println!("solutions: {}", vec.len());
                }
                Command::CalculateAllNaive => {
                    solitaire_solver::calculate_all_solutions_naive();
                }
                Command::CalculateRandomChanceSuccessRatio => {
                    let feasible = solitaire_solver::calculate_feasible_set(None);
                    let start = std::time::Instant::now();
                    let feasible = feasible.into_iter().collect();
                    let success_probabilities =
                        solitaire_solver::calculate_p_random_chance_success(feasible);
                    let p = *success_probabilities.get(&Board::default()).unwrap();
                    let percentage = p * 100.;

                    println!("took {:?}", start.elapsed());
                    println!("success probability when chosing moves at random: {percentage}%");
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
                Command::UniqueSolutions => {
                    let feasible = solitaire_solver::calculate_feasible_set(None);
                    log::info!("feasible: {}", feasible.len());
                    let solutions = solitaire_solver::all_unique_solutions(
                        Board::default(),
                        feasible.into_iter(),
                    );
                    log::info!("unique solutions: {}", solutions.len());
                }
                Command::UniquePaths => {
                    let feasible = solitaire_solver::calculate_feasible_set(None);
                    log::info!("feasible: {}", feasible.len());
                    let paths = solitaire_solver::all_unique_paths(feasible);
                    log::info!("unique paths: {}", paths.get(&Board::default()).unwrap());
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
}
