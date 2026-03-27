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
        Some(command) => match command {
            Command::CalculateAll => {
                let vec = solitaire_solver::calculate_all_solutions(args.threads);
                println!("solutions: {}", vec.len());
            }
            Command::CalculateAllNaive => {
                solitaire_solver::calculate_all_solutions_naive();
            }
            Command::CalculateRandomChanceSuccessRatio => {
                let feasible = solitaire_solver::calculate_all_solutions(None);
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
                let solutions: HashSet<Board> = solitaire_solver::calculate_all_solutions(None)
                    .into_iter()
                    .collect();
                let solutions_naive: HashSet<Board> =
                    solitaire_solver::calculate_all_solutions_naive()
                        .into_iter()
                        .collect();
                assert_eq!(solutions, solutions_naive)
            }
            Command::UniqueSolutions => {
                let feasible = solitaire_solver::calculate_all_solutions(None);
                log::info!("feasible: {}", feasible.len());
                let solutions =
                    solitaire_solver::all_unique_solutions(Board::default(), feasible.into_iter());
                log::info!("unique solutions: {}", solutions.len());
                for board in solutions[0] {
                    println!();
                    println!("{board}");
                }
            }
        },
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
