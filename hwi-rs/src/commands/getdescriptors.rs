//! `getdescriptors` — emit BIP44/49/84/86 receive + change descriptors.

use async_hwi::ledger::{HidApi, LedgerSimulator};
use bitcoin::bip32::Fingerprint;

use crate::cli::Chain;
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
    let api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    let device = open_ledger_by_fingerprint(&api, fingerprint).await?;
    do_getdescriptors(&device, fingerprint, chain, account).await
}
