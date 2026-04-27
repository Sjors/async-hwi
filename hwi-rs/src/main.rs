//! `hwi-rs`: a minimal Bitcoin Core external-signer compatible CLI.
//!
//! Drop-in subset of the Python HWI interface that Bitcoin Core invokes via
//! `-signer=<cmd>`. JSON is written to stdout. On error a JSON object
//! `{"error": "..."}` is written to stdout and the process exits non-zero.
//!
//! This first commit is a skeleton: argv parses, `--help` works, no
//! subcommands are wired up yet. Subsequent commits add the HWI verbs.

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {}

fn main() -> ExitCode {
    let _ = Args::parse();
    let mut cmd = Args::command();
    let _ = cmd.print_help();
    ExitCode::FAILURE
}
