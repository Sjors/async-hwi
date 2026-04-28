//! Ledger device support.
//!
//! Covers HID enumeration and the transport-agnostic protocol bodies
//! shared between the HID and Speculos transports. The new Ledger
//! Bitcoin app is the only firmware supported; the legacy app is not.

use async_hwi::ledger::{DeviceInfo, HidApi, Ledger, Transport, TransportHID};
use bitcoin::bip32::{DerivationPath, Fingerprint};
use bitcoin::psbt::Psbt;
use miniscript::{Descriptor, DescriptorPublicKey};

use crate::cli::Chain;
use crate::commands::{DisplayAddressReq, GetDescriptorsOut};
use crate::descriptor::{address_from_descriptor, format_descriptor, ADDR_TYPES};
use crate::policy::{build_default_policy, classify_singlesig, collect_signing_groups, SingleSig};

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

/// Find an HID-attached Ledger whose master fingerprint matches `want`.
pub async fn open_ledger_by_fingerprint(
    api: &HidApi,
    want: Fingerprint,
) -> Result<Ledger<TransportHID>, String> {
    for info in api.device_list() {
        if info.vendor_id() != LEDGER_VENDOR_ID {
            continue;
        }
        if !ledger_iface_ok(info) {
            continue;
        }
        if ledger_model(info.product_id()).is_none() {
            continue;
        }
        let Ok(device) = Ledger::<TransportHID>::connect(api, info) else {
            continue;
        };
        match async_hwi::HWI::get_master_fingerprint(&device).await {
            Ok(fp) if fp == want => return Ok(device),
            _ => continue,
        }
    }
    Err(format!("no Ledger device matching fingerprint {want:x}"))
}

// --- transport-agnostic protocol bodies --------------------------------------

pub async fn do_getdescriptors<T: Transport + Send + Sync>(
    device: &Ledger<T>,
    fingerprint: Fingerprint,
    chain: Chain,
    account: u32,
) -> Result<String, String> {
    let mut receive = Vec::new();
    let mut internal = Vec::new();

    for &(purpose, wrapper) in ADDR_TYPES {
        let base_path = format!("m/{purpose}h/{}h/{account}h", chain.coin_type());
        let derivation: DerivationPath =
            base_path.parse().map_err(|e| format!("path parse: {e}"))?;

        // If the device cannot derive a given purpose (e.g. older Ledger app
        // not supporting taproot), skip that address type and continue.
        let xpub = match async_hwi::HWI::get_extended_pubkey(device, &derivation).await {
            Ok(x) => x,
            Err(_) => continue,
        };

        let origin = format!(
            "[{:x}/{}h/{}h/{account}h]",
            fingerprint,
            purpose,
            chain.coin_type()
        );

        receive.push(format_descriptor(wrapper, &origin, &xpub.to_string(), 0));
        internal.push(format_descriptor(wrapper, &origin, &xpub.to_string(), 1));
    }

    let out = GetDescriptorsOut { receive, internal };
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

/// Fetch the xpub at `path` from the device. Mirrors HWI's `getxpub`:
/// `{"xpub": "<base58>"}`. The fingerprint is only used at the open
/// stage (HID/simulator); the device itself derives from whatever seed
/// it holds, so we don't re-check the master fingerprint here.
pub async fn do_getxpub<T: Transport + Send + Sync>(
    device: &Ledger<T>,
    path: &str,
) -> Result<String, String> {
    let derivation: DerivationPath = path.parse().map_err(|e| format!("path parse: {e}"))?;
    let xpub = async_hwi::HWI::get_extended_pubkey(device, &derivation)
        .await
        .map_err(|e| format!("get_extended_pubkey({derivation}): {e:?}"))?;
    Ok(serde_json::json!({ "xpub": xpub.to_string() }).to_string())
}

/// All four single-sig flavours go through the same path: build the
/// matching default Ledger Bitcoin app wallet policy on the fly, attach
/// it to the device session with `wallet_hmac=None`, and ask the device
/// to display the address. The new app recognises the four templates
/// `pkh(@0/**)`, `sh(wpkh(@0/**))`, `wpkh(@0/**)`, `tr(@0/**)` without
/// prior on-device registration. Mirrors what Python HWI does.
pub async fn do_displayaddress<T: Transport + Send + Sync>(
    device: Ledger<T>,
    fingerprint: Fingerprint,
    chain: Chain,
    desc: &str,
) -> Result<String, String> {
    use async_hwi::AddressScript;

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
    let acct_path: DerivationPath = format!("m/{purpose}h/{coin}h/{account}h")
        .parse()
        .map_err(|e| format!("path parse: {e}"))?;
    let xpub = async_hwi::HWI::get_extended_pubkey(&device, &acct_path)
        .await
        .map_err(|e| format!("get_extended_pubkey({acct_path}): {e:?}"))?;
    let policy = build_default_policy(purpose, fingerprint, coin, account, &xpub);

    let device = device
        .with_wallet("", &policy, None)
        .map_err(|e| format!("with_wallet({policy}): {e:?}"))?;
    async_hwi::HWI::display_address(&device, &AddressScript::Miniscript { index, change })
        .await
        .map_err(|e| format!("display_address: {e:?}"))?;

    Ok(serde_json::json!({ "address": address }).to_string())
}

/// Sign a PSBT on a Ledger. The new app insists each `sign_psbt` call be
/// scoped to one wallet policy, so we group the PSBT inputs by their
/// (BIP44 purpose, account) pair and require they all belong to one
/// group — which is what Bitcoin Core's external-signer wallet
/// (one descriptor flavour per wallet) always produces. Cross-account or
/// cross-script-type sweeps would need multiple `sign_psbt` calls and
/// therefore a fresh device session each, which is out of scope here.
pub async fn do_signtx<T: Transport + Send + Sync>(
    device: Ledger<T>,
    fingerprint: Fingerprint,
    chain: Chain,
    psbt_b64: &str,
) -> Result<String, String> {
    use bitcoin::base64::Engine as _;

    let raw = bitcoin::base64::engine::general_purpose::STANDARD
        .decode(psbt_b64.trim())
        .map_err(|e| format!("psbt base64 decode: {e}"))?;
    let mut psbt = Psbt::deserialize(&raw).map_err(|e| format!("psbt parse: {e}"))?;

    let coin = chain.coin_type();
    let groups = collect_signing_groups(&psbt, fingerprint, coin);
    let (purpose, account) = match groups.len() {
        0 => {
            return Err(format!(
                "no PSBT input has a BIP32 derivation for fingerprint {fingerprint:x} \
                 on chain {chain:?} (coin {coin})",
            ))
        }
        1 => *groups.iter().next().unwrap(),
        n => {
            return Err(format!(
                "PSBT spans {n} different (purpose, account) groups; hwi-rs currently \
                 supports one Ledger wallet policy per signtx call",
            ))
        }
    };

    let acct_path: DerivationPath = format!("m/{purpose}h/{coin}h/{account}h")
        .parse()
        .map_err(|e| format!("path parse: {e}"))?;
    let xpub = async_hwi::HWI::get_extended_pubkey(&device, &acct_path)
        .await
        .map_err(|e| format!("get_extended_pubkey({acct_path}): {e:?}"))?;
    let policy = build_default_policy(purpose, fingerprint, coin, account, &xpub);
    let device = device
        .with_wallet("", &policy, None)
        .map_err(|e| format!("with_wallet({policy}): {e:?}"))?;
    async_hwi::HWI::sign_tx(&device, &mut psbt)
        .await
        .map_err(|e| format!("sign_tx({purpose}h/{coin}h/{account}h): {e:?}"))?;

    let bytes = psbt.serialize();
    let out = bitcoin::base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(serde_json::json!({ "psbt": out }).to_string())
}

/// Substitute `@N` placeholders in a BIP388 descriptor template with the
/// caller-supplied keys, in order of `N`. Iterates from the highest
/// index down so that `@0` does not match the prefix of `@10`.
pub fn substitute_keys(template: &str, keys: &[String]) -> String {
    let mut out = template.to_string();
    for i in (0..keys.len()).rev() {
        out = out.replace(&format!("@{i}"), &keys[i]);
    }
    out
}

/// Register a BIP388 wallet policy on the device. The caller supplies
/// the template (with `@N/**` placeholders) and one key per `@N` slot —
/// exactly the shape Bitcoin Core's `RegisterPolicy` produces. We
/// re-substitute keys into the template so the underlying
/// `register_wallet` can re-extract them with its xpub regex.
pub async fn do_register<T: Transport + Send + Sync>(
    device: Ledger<T>,
    name: &str,
    desc_template: &str,
    keys: &[String],
) -> Result<String, String> {
    use bitcoin::hex::DisplayHex;

    let policy = substitute_keys(desc_template, keys);
    let hmac = async_hwi::HWI::register_wallet(&device, name, &policy)
        .await
        .map_err(|e| format!("register_wallet({name}, {policy}): {e:?}"))?
        .ok_or_else(|| "device returned no hmac".to_string())?;
    Ok(serde_json::json!({ "hmac": hmac.to_lower_hex_string() }).to_string())
}

/// Policy-based variant of `do_displayaddress`: re-attach a previously
/// registered BIP388 wallet policy (template + keys + hmac) to the
/// device session and ask it to display the address at the given
/// (change, index). Mirrors HWI PR #794's `displayaddress --policy`
/// flow.
pub async fn do_displayaddress_policy<T: Transport + Send + Sync>(
    device: Ledger<T>,
    chain: Chain,
    req: DisplayAddressReq,
) -> Result<String, String> {
    use bitcoin::hex::FromHex;

    let DisplayAddressReq::Policy {
        name,
        template,
        keys,
        hmac,
        index,
        change,
    } = req
    else {
        return Err("do_displayaddress_policy called with non-policy request".into());
    };

    let hmac_bytes = <[u8; 32]>::from_hex(&hmac).map_err(|e| format!("hmac hex decode: {e}"))?;
    let policy = substitute_keys(&template, &keys);

    let device = device
        .with_wallet(name, &policy, Some(hmac_bytes))
        .map_err(|e| format!("with_wallet({policy}): {e:?}"))?;

    // We can't always derive the address locally for arbitrary policies
    // — rust-miniscript through 13.x doesn't parse `tr(musig(...))` —
    // so trust the address the device returns over its APDU. The user
    // still confirms it on-screen, which is what the security
    // assumption rests on.
    //
    // TODO: once rust-miniscript ships `tr(musig(...))` parsing
    // (tracked at https://github.com/rust-bitcoin/rust-miniscript), we
    // can re-derive the address locally and assert equality with the
    // device-reported one as a paranoid cross-check.
    let address = device
        .display_wallet_address(change, index)
        .await
        .map_err(|e| format!("display_wallet_address: {e:?}"))?
        .assume_checked();

    Ok(serde_json::json!({
        "address": address.to_string(),
        "policy": policy,
        "index": index,
        "change": change,
        "chain": format!("{chain:?}").to_lowercase(),
    })
    .to_string())
}
