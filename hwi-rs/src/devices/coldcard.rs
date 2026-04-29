//! Coldcard device support.
//!
//! Covers HID enumeration and (when `HWI_RS_COLDCARD_SIMULATOR=1`) the
//! `coldcard-mpy` Unix-socket simulator. The transport split is provided
//! by the vendored `coldcard` crate (`coldcard-vendored/src/transport.rs`);
//! everything below is wire-protocol-agnostic.

use async_hwi::coldcard::api::{
    self as ckcc,
    protocol::{AddressFormat, DerivationPath as CkccPath, DescriptorName},
    Coldcard, SignMode,
};
use bitcoin::bip32::{DerivationPath, Fingerprint};
use bitcoin::psbt::Psbt;
use hidapi::HidApi;
use miniscript::{Descriptor, DescriptorPublicKey};

use crate::cli::Chain;
use crate::commands::{DisplayAddressReq, GetDescriptorsOut};
use crate::descriptor::{address_from_descriptor, format_descriptor, ADDR_TYPES};
use crate::policy::{classify_singlesig, collect_signing_groups, SingleSig};

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

pub fn do_getxpub(cc: &mut Coldcard, path: &str) -> Result<String, String> {
    let derivation: DerivationPath = path.parse().map_err(|e| format!("path parse: {e}"))?;
    let xpub = cc
        .xpub(Some(to_ckcc_path(&derivation)?))
        .map_err(|e| format!("coldcard xpub({derivation}): {e:?}"))?;
    Ok(serde_json::json!({ "xpub": xpub }).to_string())
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

pub fn do_signtx(
    cc: &mut Coldcard,
    fingerprint: Fingerprint,
    chain: Chain,
    psbt_b64: &str,
) -> Result<String, String> {
    use bitcoin::base64::Engine as _;

    let raw = bitcoin::base64::engine::general_purpose::STANDARD
        .decode(psbt_b64.trim())
        .map_err(|e| format!("psbt base64 decode: {e}"))?;
    let mut psbt = Psbt::deserialize(&raw).map_err(|e| format!("psbt parse: {e}"))?;

    // Sanity: PSBT must reference at least one of our keys. Coldcard will
    // sign whichever inputs it can, but if none belong to this device we
    // would loop forever in `get_signed_tx`.
    let groups = collect_signing_groups(&psbt, fingerprint, chain.coin_type());
    if groups.is_empty() {
        return Err(format!(
            "no PSBT input has a BIP32 derivation for fingerprint {fingerprint:x} \
             on chain {chain:?} (coin {})",
            chain.coin_type()
        ));
    }

    cc.sign_psbt(&psbt.serialize(), SignMode::Signed)
        .map_err(|e| format!("coldcard sign_psbt: {e:?}"))?;

    // Poll until the device produces the signed PSBT. On real hardware
    // the user has to physically press OK; in CI we drive the simulator
    // by injecting `y` keypresses through the `XKEY` USB test command
    // (handled by `usb_test_commands.do_usb_command` in the
    // `coldcard-mpy` build only). There is no useful timeout to pick
    // here — Bitcoin Core's external-signer call has its own.
    let sim = use_simulator();
    let signed = loop {
        if sim {
            // Approve any prompts that may be on screen. The firmware
            // ignores extra keypresses when no UX is waiting for input.
            let _ = cc.sim_keypress(b"y");
        }
        match cc
            .get_signed_tx()
            .map_err(|e| format!("coldcard get_signed_tx: {e:?}"))?
        {
            Some(tx) => break tx,
            None => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    };

    let mut signed = Psbt::deserialize(&signed).map_err(|e| format!("signed psbt parse: {e}"))?;
    // Merge sigs back into the caller's PSBT to preserve any other PSBT
    // fields the device may have stripped (mirrors what `async-hwi`'s
    // `Coldcard::sign_tx` does).
    for i in 0..signed.inputs.len() {
        psbt.inputs[i]
            .partial_sigs
            .append(&mut signed.inputs[i].partial_sigs);
        psbt.inputs[i]
            .tap_script_sigs
            .append(&mut signed.inputs[i].tap_script_sigs);
        if let Some(sig) = signed.inputs[i].tap_key_sig {
            psbt.inputs[i].tap_key_sig = Some(sig);
        }
    }

    let out = bitcoin::base64::engine::general_purpose::STANDARD.encode(psbt.serialize());
    Ok(serde_json::json!({ "psbt": out }).to_string())
}

// --- BIP388 / MuSig2 (policy-mode) ------------------------------------------
//
// Coldcard's BIP388 model differs from Ledger's: there is no HMAC. The
// device stores the descriptor by name (uploaded as a small JSON blob via
// `miniscript_enroll`), and identifies it on subsequent address-display
// and sign-PSBT calls by that name. We still return a (placeholder)
// 32-byte hex hmac from `register` so Bitcoin Core's wallet accepts the
// response and round-trips it back to us — we just ignore it on the
// device side and look the policy up by name. The on-screen
// confirmation that registers the wallet runs as an asynchronous UX
// flow (see `auth.maybe_enroll_xpub` in the firmware), so the `mins`
// USB command returns immediately after queueing the file. We poll
// `miniscript_get(name)` to know when the device has actually committed
// the wallet; on the simulator we drive the confirmation by injecting
// `y` keypresses through the `XKEY` test command.

/// Placeholder hmac returned from Coldcard's `register`. The Coldcard
/// BIP388 implementation has no real hmac concept (the wallet is keyed
/// purely by name on the device), but Bitcoin Core stores whatever we
/// return into `getwalletinfo.bip388[*].hmac` and re-passes it on
/// later `displayaddress`/`signtx` calls. 32 zero bytes is well-formed
/// hex and unambiguously marks "no real hmac, look up by name".
const COLDCARD_PLACEHOLDER_HMAC: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

fn make_descriptor_name(name: &str) -> Result<DescriptorName, String> {
    DescriptorName::new(name).map_err(|e| format!("coldcard descriptor name {name:?}: {e:?}"))
}

/// On the simulator, send a few `y` keypresses to dismiss whatever UX
/// modal was just opened by the previous USB command. The firmware's
/// `numpad.inject` queues the entire `args` payload as a single
/// keypress, so we issue several one-byte sends to walk through a
/// confirmation prompt. The full enroll/sign flows additionally pump
/// keypresses inside the completion-poll loop.
fn sim_dismiss(cc: &mut Coldcard) {
    if !use_simulator() {
        return;
    }
    for _ in 0..5 {
        let _ = cc.sim_keypress(b"y");
    }
}

/// Register a BIP388 wallet policy on a Coldcard. Mirrors
/// `do_register` for Ledger but returns a placeholder hmac because
/// Coldcard's BIP388 model uses the wallet name as the key.
pub fn do_register(
    cc: &mut Coldcard,
    name: &str,
    desc_template: &str,
    keys: &[String],
) -> Result<String, String> {
    use crate::devices::ledger::substitute_keys;

    // Coldcard's miniscript parser doesn't understand the BIP389 `/**`
    // shorthand; expand it to the explicit `/<0;1>/*` multipath form
    // before substituting keys.
    let policy = substitute_keys(desc_template, keys).replace("/**", "/<0;1>/*");
    // Coldcard's `miniscript_enroll` accepts a JSON blob with `name`
    // and `desc` fields (parsed by `auth.maybe_enroll_xpub`).
    let payload = serde_json::json!({ "name": name, "desc": policy }).to_string();

    cc.miniscript_enroll(payload.as_bytes())
        .map_err(|e| format!("coldcard miniscript_enroll: {e:?}"))?;

    // The `mins` USB command returns immediately with the UX queued on
    // the device. Poll `miniscript_get(name)` to confirm the wallet
    // has been committed; on the simulator, send `y` keypresses to
    // drive the on-screen confirmation. Bitcoin Core's external-signer
    // call has its own timeout, so we keep going until the device
    // either commits or returns a hard error.
    let sim = use_simulator();
    loop {
        if sim {
            // numpad.inject queues the whole `args` string as a single
            // keypress, so send one byte per call.
            let _ = cc.sim_keypress(b"y");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        match cc.miniscript_get(make_descriptor_name(name)?) {
            Ok(Some(_)) => break,
            Ok(None) => continue,
            Err(e) => return Err(format!("coldcard miniscript_get({name:?}): {e:?}")),
        }
    }

    Ok(serde_json::json!({ "hmac": COLDCARD_PLACEHOLDER_HMAC }).to_string())
}

/// Display an address for a previously registered BIP388 wallet
/// policy. Coldcard looks the wallet up by name; the supplied hmac is
/// ignored (see `COLDCARD_PLACEHOLDER_HMAC`).
pub fn do_displayaddress_policy(
    cc: &mut Coldcard,
    chain: Chain,
    req: DisplayAddressReq,
) -> Result<String, String> {
    let DisplayAddressReq::Policy {
        name,
        index,
        change,
        ..
    } = req
    else {
        return Err("do_displayaddress_policy called with non-policy request".into());
    };

    let descriptor_name = make_descriptor_name(&name)?;
    let address = cc
        .miniscript_address(descriptor_name, change, index)
        .map_err(|e| format!("coldcard miniscript_address({name:?}): {e:?}"))?;
    sim_dismiss(cc);

    Ok(serde_json::json!({
        "address": address,
        "index": index,
        "change": change,
        "chain": format!("{chain:?}").to_lowercase(),
    })
    .to_string())
}
