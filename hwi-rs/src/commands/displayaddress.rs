//! `displayaddress` — show an address on the device for confirmation.

use async_hwi::ledger::{HidApi, LedgerSimulator};
use bitcoin::bip32::Fingerprint;

use crate::cli::Chain;
use crate::devices::ledger::{do_displayaddress, open_ledger_by_fingerprint, use_simulator};
use crate::devices::mock::MockDevice;

pub async fn run_displayaddress(
    fingerprint: Fingerprint,
    chain: Chain,
    desc: &str,
) -> Result<String, String> {
    if let Some(mock) = MockDevice::from_env() {
        return mock.displayaddress(fingerprint, chain, desc);
    }
    if use_simulator() {
        let device = LedgerSimulator::try_connect()
            .await
            .map_err(|e| format!("speculos connect: {e:?}"))?;
        return do_displayaddress(device, fingerprint, chain, desc).await;
    }
    let api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    let device = open_ledger_by_fingerprint(&api, fingerprint).await?;
    do_displayaddress(device, fingerprint, chain, desc).await
}
