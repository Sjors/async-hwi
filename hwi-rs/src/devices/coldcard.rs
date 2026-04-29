//! Coldcard device support.
//!
//! Covers HID enumeration and (when `HWI_RS_COLDCARD_SIMULATOR=1`) the
//! `coldcard-mpy` Unix-socket simulator. The transport split is provided
//! by the vendored `coldcard` crate (`coldcard-vendored/src/transport.rs`);
//! everything below is wire-protocol-agnostic.

use async_hwi::coldcard::api::{
    self as ckcc,
    protocol::{AddressFormat, DerivationPath as CkccPath},
    Coldcard,
};
use bitcoin::bip32::{DerivationPath, Fingerprint};
use hidapi::HidApi;
use miniscript::{Descriptor, DescriptorPublicKey};

use crate::cli::Chain;
use crate::commands::GetDescriptorsOut;
use crate::descriptor::{address_from_descriptor, format_descriptor, ADDR_TYPES};
use crate::policy::{classify_singlesig, SingleSig};

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

/// Find a HID-attached Coldcard whose master fingerprint matches `want`.
///
/// Coldcards always expose a single HID interface, so this is a simple
/// vid/pid filter followed by an open + xpub fetch. The fingerprint
/// returned by the post-handshake `XpubInfo` is the master fingerprint
/// (already a hash160 of the master pubkey), so no extra round trip is
/// needed.
pub fn open_coldcard_by_fingerprint(
    api: &mut HidApi,
    want: Fingerprint,
) -> Result<Coldcard, String> {
    let mut ck_api = ckcc::Api::from_borrowed(api);
    let serials = ck_api
        .detect()
        .map_err(|e| format!("coldcard detect: {e:?}"))?;
    for sn in serials {
        let (cc, info) = match ck_api.open(&sn, None) {
            Ok(x) => x,
            Err(_) => continue,
        };
        if let Some(info) = info {
            if Fingerprint::from(info.fingerprint) == want {
                return Ok(cc);
            }
        }
    }
    Err(format!("no Coldcard device matching fingerprint {want:x}"))
}

// --- transport-agnostic protocol bodies --------------------------------------

fn to_ckcc_path(path: &DerivationPath) -> Result<CkccPath, String> {
    let s = path.to_string();
    let s = if s.starts_with("m/") || s == "m" {
        s
    } else {
        format!("m/{s}")
    };
    CkccPath::new(&s).map_err(|e| format!("coldcard path {s}: {e:?}"))
}

fn ckcc_addr_fmt(purpose: u32) -> Result<AddressFormat, String> {
    match purpose {
        44 => Ok(AddressFormat::P2PKH),
        49 => Ok(AddressFormat::P2WPKH_P2SH),
        84 => Ok(AddressFormat::P2WPKH),
        86 => Ok(AddressFormat::P2TR),
        other => Err(format!("unsupported BIP44 purpose for coldcard: {other}")),
    }
}

pub fn do_getdescriptors(
    cc: &mut Coldcard,
    fingerprint: Fingerprint,
    chain: Chain,
    account: u32,
) -> Result<String, String> {
    let mut receive = Vec::new();
    let mut internal = Vec::new();

    for &(purpose, wrapper) in ADDR_TYPES {
        let coin = chain.coin_type();
        let path: DerivationPath = format!("m/{purpose}h/{coin}h/{account}h")
            .parse()
            .map_err(|e| format!("path parse: {e}"))?;
        let xpub_str = cc
            .xpub(Some(to_ckcc_path(&path)?))
            .map_err(|e| format!("coldcard xpub({path}): {e:?}"))?;
        let origin = format!("[{fingerprint:x}/{purpose}h/{coin}h/{account}h]");
        receive.push(format_descriptor(wrapper, &origin, &xpub_str, 0));
        internal.push(format_descriptor(wrapper, &origin, &xpub_str, 1));
    }

    let out = GetDescriptorsOut { receive, internal };
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

pub fn do_displayaddress(cc: &mut Coldcard, chain: Chain, desc: &str) -> Result<String, String> {
    let parsed: Descriptor<DescriptorPublicKey> =
        desc.parse().map_err(|e| format!("descriptor parse: {e}"))?;
    let address = address_from_descriptor(desc, chain)?;

    let SingleSig {
        purpose,
        account,
        change,
        index,
    } = classify_singlesig(&parsed, chain.coin_type())?;

    let coin = chain.coin_type();
    let subpath: DerivationPath = format!(
        "m/{purpose}h/{coin}h/{account}h/{}/{index}",
        if change { 1 } else { 0 }
    )
    .parse()
    .map_err(|e| format!("path build: {e}"))?;

    let _shown = cc
        .address(to_ckcc_path(&subpath)?, ckcc_addr_fmt(purpose)?)
        .map_err(|e| format!("coldcard address({subpath}): {e:?}"))?;

    // On the simulator the address is rendered as a modal that the user
    // would normally dismiss with `y`. Inject the keypress so the device
    // returns to the home screen and is ready for the next USB command;
    // production firmware ignores `XKEY` (it's a `coldcard-mpy`-only
    // test command).
    if use_simulator() {
        let _ = cc.sim_keypress(b"y");
    }

    Ok(serde_json::json!({ "address": address }).to_string())
}
