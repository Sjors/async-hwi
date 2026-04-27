//! Argument parsing for the `hwi-rs` external-signer CLI.
//!
//! The flag set is intentionally a subset of the Python HWI CLI — Bitcoin
//! Core only ever invokes a handful of subcommands and a fixed pair of
//! global flags (`--fingerprint`, `--chain`).

use bitcoin::{bip32::Fingerprint, Network};
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

impl Chain {
    /// BIP44 coin type. 0 for mainnet, 1 for everything else.
    pub fn coin_type(self) -> u32 {
        match self {
            Chain::Main => 0,
            _ => 1,
        }
    }

    /// Network used for address encoding. Testnet3 / testnet4 / signet all
    /// use the `tb1`/`m`/`n` prefixes; regtest uses `bcrt1`.
    pub fn network(self) -> Network {
        match self {
            Chain::Main => Network::Bitcoin,
            Chain::Test => Network::Testnet,
            Chain::Testnet4 => Network::Testnet4,
            Chain::Regtest => Network::Regtest,
            Chain::Signet => Network::Signet,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// List all detected devices as a JSON array.
    Enumerate,

    /// Return BIP44/49/84/86 receive and internal descriptors for the given
    /// account, as `{"receive": [...], "internal": [...]}`.
    Getdescriptors {
        #[arg(long, default_value_t = 0)]
        account: u32,
    },
}
