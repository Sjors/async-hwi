//! `getxpub` — return the xpub at a BIP32 path.

use async_hwi::ledger::{HidApi, LedgerSimulator};
use bitcoin::bip32::Fingerprint;

use crate::cli::Chain;
use crate::devices::ledger::{do_getxpub, open_ledger_by_fingerprint, use_simulator};
use crate::devices::mock::MockDevice;

pub async fn run_getxpub(
    fingerprint: Fingerprint,
    chain: Chain,
    path: &str,
) -> Result<String, String> {
    if let Some(mock) = MockDevice::from_env() {
        return mock.getxpub(fingerprint, chain, path);
    }
    // Enable on-device xpub confirmation. The Ledger Bitcoin app
    // refuses to derive non-standard paths (anything outside
    // BIP44/49/84/86 + BIP48-multisig) without an explicit user
    // confirmation, returning `NotSupported`. Anyone calling
    // `getxpub` directly is asking for a custom path by definition,
    // so always opt in to the prompt.
    if use_simulator() {
        let device = LedgerSimulator::try_connect()
            .await
            .map_err(|e| format!("speculos connect: {e:?}"))?
            .display_xpub(true)
            .map_err(|e| format!("display_xpub: {e:?}"))?;
        return do_getxpub(&device, path).await;
    }
    let api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    let device = open_ledger_by_fingerprint(&api, fingerprint)
        .await?
        .display_xpub(true)
        .map_err(|e| format!("display_xpub: {e:?}"))?;
    do_getxpub(&device, path).await
}
