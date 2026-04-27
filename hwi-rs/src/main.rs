//! `hwi-rs`: a minimal Bitcoin Core external-signer compatible CLI.
//!
//! Drop-in subset of the Python HWI interface that Bitcoin Core invokes via
//! `-signer=<cmd>`. JSON is written to stdout. On error a JSON object
//! `{"error": "..."}` is written to stdout and the process exits non-zero.
//!
//! Currently supported:
//!   * device:      Ledger (new app only; legacy not supported)
//!   * subcommands: `enumerate`, `getdescriptors`
//!
//! Source layout:
//!   * [`cli`] — argv parsing
//!   * [`devices`] — per-device modules (ledger, mock); enumeration,
//!     transport-agnostic protocol bodies, JSON shape
//!   * [`descriptor`] — checksummed descriptor construction
//!   * [`commands`] — per-subcommand `run_*` dispatch (mock → simulator → HID)

mod cli;
mod commands;
mod descriptor;
mod devices;

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use cli::{Args, Command};

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let Some(command) = args.command else {
        let mut cmd = Args::command();
        let _ = cmd.print_help();
        return ExitCode::FAILURE;
    };

    let result = match command {
        Command::Enumerate => commands::run_enumerate().await,
        Command::Getdescriptors { account } => match args.fingerprint {
            Some(fp) => commands::run_getdescriptors(fp, args.chain, account).await,
            None => Err("a fingerprint is required for this command".into()),
        },
    };

    match result {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(e) => commands::emit_error(e),
    }
}
