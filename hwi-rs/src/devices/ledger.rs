//! Ledger device support.
//!
//! Covers HID enumeration helpers shared by the per-subcommand dispatch
//! modules. The new Ledger Bitcoin app is the only firmware supported;
//! the legacy app is not.

use async_hwi::ledger::DeviceInfo;

pub const LEDGER_VENDOR_ID: u16 = 0x2c97;

/// True when `HWI_RS_LEDGER_SIMULATOR=1` is set in the environment. In that
/// mode every subcommand skips HID and talks to a Speculos instance over
/// its APDU TCP port (default 127.0.0.1:9999). Used by the simulator
/// integration test in CI; see `tests/run-core-scenario-speculos.sh`.
pub fn use_simulator() -> bool {
    std::env::var("HWI_RS_LEDGER_SIMULATOR").ok().as_deref() == Some("1")
}

/// Map a Ledger USB product ID to the model string HWI exposes.
///
/// The new-app product IDs use the high byte for the model.
pub fn ledger_model(product_id: u16) -> Option<&'static str> {
    match product_id >> 8 {
        0x10 => Some("ledger_nano_s"),
        0x40 => Some("ledger_nano_x"),
        0x50 => Some("ledger_nano_s_plus"),
        0x60 => Some("ledger_stax"),
        0x70 => Some("ledger_flex"),
        _ => None,
    }
}

/// True if the HID interface looks like a Ledger Bitcoin app endpoint
/// (mirrors HWI's filter).
pub fn ledger_iface_ok(info: &DeviceInfo) -> bool {
    info.interface_number() == 0 || info.usage_page() == 0xffa0
}
