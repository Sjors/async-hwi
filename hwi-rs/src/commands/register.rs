//! `register` — register a BIP388 wallet policy on the device.
//!
//! Bitcoin Core (Sjors's MuSig2 branch, PR #91) calls this from
//! `registerpolicy` for any non-default policy:
//!
//!   <cmd> --fingerprint FP --chain CHAIN register \
//!         --name NAME --desc TEMPLATE --key KEY1 [--key KEY2 ...]
//!
//! and parses `{"hmac": "<hex>"}` out of stdout.

use async_hwi::ledger::{HidApi, LedgerSimulator};
use bitcoin::bip32::Fingerprint;

use crate::cli::Chain;
use crate::devices::ledger::{do_register, open_ledger_by_fingerprint, use_simulator};
use crate::devices::mock::MockDevice;

pub async fn run_register(
    fingerprint: Fingerprint,
    chain: Chain,
    name: &str,
    desc_template: &str,
    keys: &[String],
) -> Result<String, String> {
    if let Some(mock) = MockDevice::from_env() {
        return mock.register(fingerprint, chain, name, desc_template, keys);
    }
    if use_simulator() {
        let device = LedgerSimulator::try_connect()
            .await
            .map_err(|e| format!("speculos connect: {e:?}"))?;
        return do_register(device, name, desc_template, keys).await;
    }
    let api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    let device = open_ledger_by_fingerprint(&api, fingerprint).await?;
    do_register(device, name, desc_template, keys).await
}
