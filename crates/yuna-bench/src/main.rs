//! `yuna-bench`: the rivals' bench. Its command line is in [`yuna_bench::cli`].

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    yuna_bench::cli::main(&arguments)
}
