fn main() {
    let solve_all = std::env::args().any(|a| &a == "-a");
    let solve_first = std::env::args().any(|a| &a == "-s");
    if solve_all {
        solitaire_solver::calculate_all_solutions();
        return;
    }
    if solve_first {
        let solution = solitaire_solver::calculate_first_solution();
        solitaire_solver::print_solution(solution);
        return;
    }
    solitaire_game::run();
}
