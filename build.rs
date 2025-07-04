use std::error::Error;

use solitaire_solver;

fn main() -> Result<(), Box<dyn Error>> {
    let solutions = solitaire_solver::calculate_all_solutions();
    solitaire_solver::write_solutions(solutions, "solutions.dat.br")?;
    Ok(())
}
