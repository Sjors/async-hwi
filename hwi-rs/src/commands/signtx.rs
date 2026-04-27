//! `signtx` — sign a base64 PSBT on the device, returning the merged PSBT.

use async_hwi::ledger::{HidApi, LedgerSimulator};
use bitcoin::bip32::Fingerprint;

use crate::cli::Chain;
use crate::devices::ledger::{do_signtx, open_ledger_by_fingerprint, use_simulator};
use crate::devices::mock::MockDevice;

pub async fn run_signtx(
    fingerprint: Fingerprint,
    chain: Chain,
    psbt_b64: &str,
) -> Result<String, String> {
    if let Some(mock) = MockDevice::from_env() {
        return mock.signtx(fingerprint, chain, psbt_b64);
    }
    if use_simulator() {
        let device = LedgerSimulator::try_connect()
            .await
            .map_err(|e| format!("speculos connect: {e:?}"))?;
        return do_signtx(device, fingerprint, chain, psbt_b64).await;
    }
    let api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    let device = open_ledger_by_fingerprint(&api, fingerprint).await?;
    do_signtx(device, fingerprint, chain, psbt_b64).await
}
