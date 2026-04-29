//! Coldcard device support.
//!
//! Covers HID enumeration and (when `HWI_RS_COLDCARD_SIMULATOR=1`) the
//! `coldcard-mpy` Unix-socket simulator. The transport split is provided
//! by the vendored `coldcard` crate (`coldcard-vendored/src/transport.rs`);
//! everything below is wire-protocol-agnostic.

use async_hwi::coldcard::api::Coldcard;
use bitcoin::bip32::Fingerprint;

/// Default path of the headless `coldcard-mpy` simulator's Unix datagram
/// socket. Matches the upstream firmware's hard-coded location.
pub const SIMULATOR_SOCKET: &str = "/tmp/ckcc-simulator.sock";

/// True when `HWI_RS_COLDCARD_SIMULATOR=1` is set in the environment. In
/// that mode every subcommand bypasses HID enumeration and connects to a
/// running `coldcard-mpy` instance over its Unix datagram socket. Used by
/// the simulator integration test in CI; see
/// `tests/run-core-scenario-coldcard.sh`.
pub fn use_simulator() -> bool {
    std::env::var("HWI_RS_COLDCARD_SIMULATOR").ok().as_deref() == Some("1")
}

/// Map a Coldcard `version()` string to the model name HWI exposes.
///
/// `Coldcard::version()` returns a multi-line blob; the last non-empty
/// line carries the hardware variant (`mk4`, `q1`, `mk5` for the
/// simulator, ...). Anything unparsable is reported as `coldcard` so
/// enumeration never fails just because a new hardware model shipped.
pub fn coldcard_model(version: &str) -> String {
    version
        .lines()
        .map(str::trim)
        .rfind(|s| !s.is_empty())
        .map(|s| format!("coldcard_{s}"))
        .unwrap_or_else(|| "coldcard".to_string())
}

/// Open a connection to the running `coldcard-mpy` simulator over its
/// Unix datagram socket (default path: [`SIMULATOR_SOCKET`]).
///
/// Returns the `Coldcard` handle plus its master fingerprint, fetched via
/// the post-handshake `XpubInfo`. The simulator has no factory key, so
/// MITM verification is meaningless and is intentionally not invoked.
pub fn open_simulator() -> Result<(Coldcard, Fingerprint), String> {
    let path = std::env::var("HWI_RS_COLDCARD_SIMULATOR_SOCKET")
        .unwrap_or_else(|_| SIMULATOR_SOCKET.to_string());
    let (cc, info) = Coldcard::open_simulator(&path, None)
        .map_err(|e| format!("coldcard simulator connect ({path}): {e:?}"))?;
    let info = info
        .ok_or_else(|| "coldcard simulator returned no xpub: device not initialised".to_string())?;
    Ok((cc, Fingerprint::from(info.fingerprint)))
}
