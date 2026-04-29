//! Argument parsing for the `hwi-rs` external-signer CLI.
//!
//! The flag set is intentionally a subset of the Python HWI CLI — Bitcoin
//! Core only ever invokes a handful of subcommands and a fixed pair of
//! global flags (`--fingerprint`, `--chain`).

use bitcoin::{bip32::Fingerprint, Network};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None, max_term_width = 100)]
pub struct Args {
    /// Master fingerprint of the device to act on (hex). Required for all
    /// subcommands except `enumerate`.
    #[arg(long, global = true, value_parser = clap::value_parser!(Fingerprint))]
    pub fingerprint: Option<Fingerprint>,

    /// Bitcoin chain. Matches HWI's `--chain` flag.
    #[arg(long, global = true, value_enum, default_value_t = Chain::Main)]
    pub chain: Chain,

    /// Read the subcommand line from stdin instead of argv. Bitcoin Core
    /// uses this for `signtx` to avoid putting a multi-kilobyte base64 PSBT
    /// in argv. The first stdin line is parsed in the same shape as argv
    /// would have been (e.g. `signtx <base64>`).
    #[arg(long, global = true, default_value_t = false)]
    pub stdin: bool,

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

    /// Get the extended public key derived at the given BIP32 path,
    /// echoed back as `{"xpub": "..."}`. Mirrors HWI's `getxpub`. Useful
    /// for fetching custom-path keys (e.g. BIP87 multisig account keys
    /// at `m/87'/1'/0'`) that `getdescriptors` does not cover.
    Getxpub {
        /// BIP32 derivation path (e.g. `m/87'/1'/0'` or
        /// `m/87h/1h/0h`).
        path: String,
    },

    /// Display an address derived from the given descriptor on the device,
    /// and echo it back as `{"address": "..."}`. The descriptor is the one
    /// Bitcoin Core produces via `InferDescriptor` for a single scriptPubKey,
    /// so it has no wildcards.
    Displayaddress {
        #[arg(long)]
        desc: String,
    },

    /// Sign a base64 PSBT and echo back the signed PSBT (also base64) as
    /// `{"psbt": "..."}`. Typically read from stdin via `--stdin` since
    /// PSBTs can be larger than the argv limit.
    Signtx { psbt: String },
}
