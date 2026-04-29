//! `getdescriptors` — emit BIP44/49/84/86 receive + change descriptors.

use async_hwi::ledger::{HidApi, LedgerSimulator};
use bitcoin::bip32::Fingerprint;

use crate::cli::Chain;
use crate::devices::coldcard::{
    do_getdescriptors as cc_getdescriptors, open_coldcard_by_fingerprint,
    open_simulator as open_cc_simulator, use_simulator as use_cc_simulator,
};
use crate::devices::ledger::{do_getdescriptors, open_ledger_by_fingerprint, use_simulator};
use crate::devices::mock::MockDevice;

pub async fn run_getdescriptors(
    fingerprint: Fingerprint,
    chain: Chain,
    account: u32,
) -> Result<String, String> {
    if let Some(mock) = MockDevice::from_env() {
        return mock.getdescriptors(fingerprint, chain, account);
    }
    if use_simulator() {
        let device = LedgerSimulator::try_connect()
            .await
            .map_err(|e| format!("speculos connect: {e:?}"))?;
        return do_getdescriptors(&device, fingerprint, chain, account).await;
    }
    if use_cc_simulator() {
        let (mut cc, _) = open_cc_simulator()?;
        return cc_getdescriptors(&mut cc, fingerprint, chain, account);
    }
    let mut api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    if let Ok(mut cc) = open_coldcard_by_fingerprint(&mut api, fingerprint) {
        return cc_getdescriptors(&mut cc, fingerprint, chain, account);
    }
    let device = open_ledger_by_fingerprint(&api, fingerprint).await?;
    do_getdescriptors(&device, fingerprint, chain, account).await
}
