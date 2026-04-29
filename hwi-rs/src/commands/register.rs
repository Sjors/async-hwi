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
use crate::devices::coldcard::{
    do_register as cc_register, open_coldcard_by_fingerprint, open_simulator as open_cc_simulator,
    use_simulator as use_cc_simulator,
};
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
    if use_cc_simulator() {
        let (mut cc, _) = open_cc_simulator()?;
        return cc_register(&mut cc, name, desc_template, keys);
    }
    let mut api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    if let Ok(mut cc) = open_coldcard_by_fingerprint(&mut api, fingerprint) {
        return cc_register(&mut cc, name, desc_template, keys);
    }
    let device = open_ledger_by_fingerprint(&api, fingerprint).await?;
    do_register(device, name, desc_template, keys).await
}
