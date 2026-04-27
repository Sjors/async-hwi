//! Ledger device support.
//!
//! Covers HID enumeration and the transport-agnostic protocol bodies
//! shared between the HID and Speculos transports. The new Ledger
//! Bitcoin app is the only firmware supported; the legacy app is not.

use async_hwi::ledger::{DeviceInfo, HidApi, Ledger, Transport, TransportHID};
use bitcoin::bip32::{DerivationPath, Fingerprint};
use miniscript::{Descriptor, DescriptorPublicKey};

use crate::cli::Chain;
use crate::commands::GetDescriptorsOut;
use crate::descriptor::{address_from_descriptor, format_descriptor, ADDR_TYPES};
use crate::policy::{build_default_policy, classify_singlesig, SingleSig};

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
