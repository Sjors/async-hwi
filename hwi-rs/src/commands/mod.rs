//! Subcommand dispatch.
//!
//! Each `run_*` picks one of mock → simulator → HID and delegates the
//! protocol body to the matching device module under [`crate::devices`].

use std::process::ExitCode;

use serde::{Deserialize, Serialize};

mod enumerate;
mod getdescriptors;

pub use enumerate::run_enumerate;
pub use getdescriptors::run_getdescriptors;

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
