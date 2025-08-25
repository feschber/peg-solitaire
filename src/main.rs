fn main() {
    if std::env::args().any(|a| &a == "-a") {
        solitaire_solver::calculate_all_solutions();
    } else if std::env::args().any(|a| &a == "-n") {
        solitaire_solver::calculate_all_solutions_naive();
    } else if std::env::args().any(|a| &a == "-s") {
        let solution = solitaire_solver::calculate_first_solution();
        solitaire_solver::print_solution(solution);
    } else {
        #[cfg(feature = "game")]
        solitaire_game::run();

        #[cfg(not(feature = "game"))]
        {
            eprintln!("\"game\" feature not enabled!");
            std::process::exit(1)
        }
    }
}
