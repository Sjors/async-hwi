//! Per-device modules.
//!
//! Each submodule wraps one physical or virtual signer:
//!   * [`ledger`] — real or Speculos-emulated Ledger Bitcoin app
//!   * [`mock`]   — in-process software signer used by CI
//!
//! Shared types (currently just the JSON shape Bitcoin Core consumes
//! from `enumerate`) live here at the parent level.

use serde::Serialize;

pub mod ledger;
pub mod mock;

/// JSON shape Bitcoin Core consumes from `enumerate`. Mirrors HWI's output.
#[derive(Serialize)]
pub struct DeviceEntry {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub model: String,
    pub label: Option<String>,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    pub needs_pin_sent: bool,
    pub needs_passphrase_sent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
