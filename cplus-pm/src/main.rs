//! `cplus-pm` — the standalone package-manager binary. A thin wrapper over
//! [`cplus_pm::cli::run`], which is the same dispatcher that backs `cpc pm`.
//! Kept as a compatibility alias; new usage is `cpc pm ...`.

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    match cplus_pm::cli::run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
