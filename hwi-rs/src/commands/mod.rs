//! Subcommand dispatch.
//!
//! Each `run_*` picks one of mock → simulator → HID and delegates the
//! protocol body to the matching device module under [`crate::devices`].

use std::process::ExitCode;

mod enumerate;

pub use enumerate::run_enumerate;

pub fn emit_error(e: String) -> ExitCode {
    let body = serde_json::json!({ "error": e });
    println!("{body}");
    ExitCode::FAILURE
}
