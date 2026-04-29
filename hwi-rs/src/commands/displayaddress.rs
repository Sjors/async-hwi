//! `displayaddress` — show an address on the device for confirmation.

use async_hwi::ledger::{HidApi, LedgerSimulator};
use bitcoin::bip32::Fingerprint;

use crate::cli::Chain;
use crate::devices::coldcard::{
    do_displayaddress as cc_displayaddress, open_coldcard_by_fingerprint,
    open_simulator as open_cc_simulator, use_simulator as use_cc_simulator,
};
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
    if use_cc_simulator() {
        let (mut cc, _) = open_cc_simulator()?;
        return cc_displayaddress(&mut cc, chain, desc);
    }
    let mut api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    if let Ok(mut cc) = open_coldcard_by_fingerprint(&mut api, fingerprint) {
        return cc_displayaddress(&mut cc, chain, desc);
    }
    let device = open_ledger_by_fingerprint(&api, fingerprint).await?;
    do_displayaddress(device, fingerprint, chain, desc).await
}
