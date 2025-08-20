fn main() {
    if std::env::args().any(|a| &a == "-a") {
        solitaire_solver::calculate_all_solutions();
    } else if std::env::args().any(|a| &a == "-s") {
        let solution = solitaire_solver::calculate_first_solution();
        solitaire_solver::print_solution(solution);
    } else {
        solitaire_game::run();
    }
}
