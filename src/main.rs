use std::{collections::HashSet, num::NonZero};

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
    /// iteration for startup, first-touch faulting ~40MB, and mimalloc's teardown
    /// purge - which together are a couple of percent of a run, all of it noise
    /// relative to the loops one is usually trying to measure. Repeating in-process
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
