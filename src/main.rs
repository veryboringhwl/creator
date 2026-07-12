use std::process::ExitCode;

use clap::Parser;
use creator::cli;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match cli::dispatch(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
