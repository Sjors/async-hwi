//! `hwi-rs`: a minimal Bitcoin Core external-signer compatible CLI.
//!
//! Drop-in subset of the Python HWI interface that Bitcoin Core invokes via
//! `-signer=<cmd>`. JSON is written to stdout. On error a JSON object
//! `{"error": "..."}` is written to stdout and the process exits non-zero.
//!
//! Currently supported:
//!   * device:      Ledger (new app only; legacy not supported)
//!   * subcommands: `enumerate`, `getdescriptors`, `displayaddress`, `signtx`
//!
//! Source layout:
//!   * [`cli`] — argv parsing
//!   * [`devices`] — per-device modules (ledger, mock); enumeration,
//!     transport-agnostic protocol bodies, JSON shape
//!   * [`descriptor`] — definite-descriptor inspection + BIP380 checksum
//!   * [`policy`] — mapping descriptors / PSBT derivations to Ledger
//!     default single-sig wallet policies
//!   * [`commands`] — per-subcommand `run_*` dispatch (mock → simulator → HID)

mod cli;
mod commands;
mod descriptor;
mod devices;
mod policy;

use std::process::ExitCode;

use clap::{CommandFactory, Parser};

use cli::{Args, Command};

#[tokio::main]
async fn main() -> ExitCode {
    let mut args = Args::parse();
    if args.stdin {
        // `--stdin` mode: re-parse the subcommand line from stdin so Core
        // can pass a multi-kilobyte base64 PSBT to `signtx` without
        // tripping the argv length limit.
        match commands::read_stdin_command(&args) {
            Ok(re) => args = re,
            Err(e) => return commands::emit_error(e),
        }
    }
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
        Command::Getxpub { path } => match args.fingerprint {
            Some(fp) => commands::run_getxpub(fp, args.chain, &path).await,
            None => Err("a fingerprint is required for this command".into()),
        },
        Command::Displayaddress {
            desc,
            policy_name,
            policy_desc,
            key,
            hmac,
            index,
            change,
        } => match args.fingerprint {
            Some(fp) => {
                let req = match (desc, policy_name, policy_desc, hmac, index) {
                    (Some(d), None, None, None, None) => {
                        commands::DisplayAddressReq::SingleSig { desc: d }
                    }
                    (None, Some(name), Some(template), Some(hmac), Some(index)) => {
                        commands::DisplayAddressReq::Policy {
                            name,
                            template,
                            keys: key,
                            hmac,
                            index,
                            change,
                        }
                    }
                    _ => {
                        return commands::emit_error(
                            "displayaddress requires either --desc, or all of \
                             --policy-name --policy-desc --key --hmac --index"
                                .into(),
                        )
                    }
                };
                commands::run_displayaddress(fp, args.chain, req).await
            }
            None => Err("a fingerprint is required for this command".into()),
        },
        Command::Register { name, desc, key } => match args.fingerprint {
            Some(fp) => commands::run_register(fp, args.chain, &name, &desc, &key).await,
            None => Err("a fingerprint is required for this command".into()),
        },
        Command::Signtx { psbt } => match args.fingerprint {
            Some(fp) => commands::run_signtx(fp, args.chain, &psbt).await,
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
