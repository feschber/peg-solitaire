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
}

fn main() {
    let args = Args::parse();
    match args.command {
        Some(command) => match command {
            Command::CalculateAll => {
                solitaire_solver::calculate_all_solutions(args.threads.into());
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
    if std::env::args().any(|a| &a == "-a") {
    } else if std::env::args().any(|a| &a == "-n") {
    } else if std::env::args().any(|a| &a == "-c") {
    } else if std::env::args().any(|a| &a == "-s") {
        let solution = solitaire_solver::calculate_first_solution();
        solitaire_solver::print_solution(solution);
    }
}
