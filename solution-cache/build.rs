use std::{
    env,
    error::Error,
    fs,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use solitaire_solver::{Board, calculate_all_solutions};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo::rerun-if-changed=../solitaire-solver");
    let solutions = calculate_all_solutions();
    let out_dir = env::var("OUT_DIR")?;
    let out_dir = PathBuf::from_str(&out_dir)?;
    let solution_file = out_dir.join("solutions.dat.br");
    write_solutions(solutions, solution_file)?;
    Ok(())
}

fn write_solutions<P>(solutions: Vec<Board>, p: P) -> io::Result<()>
where
    P: AsRef<Path>,
{
    let solutions = solutions
        .into_iter()
        .map(|b| b.to_compressed_repr())
        .collect::<Vec<_>>();
    // solutions with the first bit set
    let sol_gt_u32 = solutions
        .iter()
        .filter(|&b| *b > u32::MAX as u64)
        .map(|&b| b as u32)
        .collect::<Vec<_>>();
    // solutions with the first bit not set
    let sol_lt_u32 = solutions
        .iter()
        .filter(|&b| *b <= u32::MAX as u64)
        .map(|&b| b as u32)
        .collect::<Vec<_>>();
    let count_gt_u32 = sol_gt_u32.len() as u32;
    let f = fs::File::create(p)?;
    let f = BufWriter::new(f);
    let mut compressor = brotli::CompressorWriter::new(f, 4096, 11, 22);
    let count_gt_u32 = count_gt_u32.to_le_bytes();
    compressor.write_all(&count_gt_u32)?;
    for b in sol_gt_u32 {
        let bytes = b.to_le_bytes();
        compressor.write_all(&bytes)?;
    }
    for b in sol_lt_u32 {
        let bytes = b.to_le_bytes();
        compressor.write_all(&bytes)?;
    }
    Ok(())
}
