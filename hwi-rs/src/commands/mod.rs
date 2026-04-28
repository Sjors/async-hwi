//! Subcommand dispatch.
//!
//! Each `run_*` picks one of mock → simulator → HID and delegates the
//! protocol body to the matching device module under [`crate::devices`].

use std::io::BufRead;
use std::process::ExitCode;

use serde::{Deserialize, Serialize};

mod displayaddress;
mod enumerate;
mod getdescriptors;
mod getxpub;
mod register;
mod signtx;

pub use displayaddress::{run_displayaddress, DisplayAddressReq};
pub use enumerate::run_enumerate;
pub use getdescriptors::run_getdescriptors;
pub use getxpub::run_getxpub;
pub use register::run_register;
pub use signtx::{run_signtx, SignTxReq};

#[derive(Serialize, Deserialize)]
pub struct GetDescriptorsOut {
    pub receive: Vec<String>,
    pub internal: Vec<String>,
}

pub fn emit_error(e: String) -> ExitCode {
    let body = serde_json::json!({ "error": e });
    println!("{body}");
    ExitCode::FAILURE
}

/// Read one line from stdin and re-parse it through clap as if it had
/// been the full argv (positional subcommand and its flags). Bitcoin
/// Core uses `--stdin signtx` and feeds the base64 PSBT as the next
/// stdin line, so users get the same flag parsing rules as on the
/// command line. Returns the freshly-parsed [`crate::cli::Args`].
pub fn read_stdin_command(base: &crate::cli::Args) -> Result<crate::cli::Args, String> {
    use clap::Parser;

    let mut line = String::new();
    std::io::BufReader::new(std::io::stdin())
        .read_line(&mut line)
        .map_err(|e| format!("stdin read: {e}"))?;
    let line = line.trim_end_matches(['\r', '\n']);

    // Split the stdin line on whitespace into argv-style tokens. We
    // intentionally do NOT use shell-style quote handling here:
    // BIP32-style key origin paths like `[fp/87'/1'/0']tpubD...` embed
    // single quotes that a shell tokeniser would treat as quotes,
    // mangling subsequent `--key`/`--policy-desc` arguments. None of
    // the values we exchange contain whitespace (PSBTs are base64,
    // descriptors and keys never contain spaces), so a plain
    // whitespace split is both correct and unambiguous. Re-prepend a
    // fake binary name so clap's positional parsing matches the argv
    // flow.
    let mut argv: Vec<String> = vec!["hwi-rs".to_string()];
    if let Some(fp) = base.fingerprint {
        argv.push("--fingerprint".into());
        argv.push(format!("{fp:x}"));
    }
    argv.push("--chain".into());
    argv.push(format!("{:?}", base.chain).to_lowercase());
    argv.extend(line.split_whitespace().map(str::to_string));

    crate::cli::Args::try_parse_from(argv).map_err(|e| format!("stdin args parse: {e}"))
}
