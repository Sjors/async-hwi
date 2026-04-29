//! Argument parsing for the `hwi-rs` external-signer CLI.
//!
//! The flag set is intentionally a subset of the Python HWI CLI — Bitcoin
//! Core only ever invokes a handful of subcommands and a fixed pair of
//! global flags (`--fingerprint`, `--chain`).

use bitcoin::bip32::Fingerprint;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Master fingerprint of the device to act on (hex). Required for all
    /// subcommands except `enumerate`.
    #[arg(long, global = true, value_parser = clap::value_parser!(Fingerprint))]
    pub fingerprint: Option<Fingerprint>,

    /// Bitcoin chain. Matches HWI's `--chain` flag.
    #[arg(long, global = true, value_enum, default_value_t = Chain::Main)]
    pub chain: Chain,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum Chain {
    Main,
    Test,
    Testnet4,
    Regtest,
    Signet,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// List all detected devices as a JSON array.
    Enumerate,
}
