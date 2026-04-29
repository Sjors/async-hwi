//! `getxpub` — return the xpub at a BIP32 path.

use async_hwi::ledger::{HidApi, LedgerSimulator};
use bitcoin::bip32::Fingerprint;

use crate::cli::Chain;
use crate::devices::coldcard::{
    do_getxpub as cc_getxpub, open_coldcard_by_fingerprint, open_simulator as open_cc_simulator,
};
use crate::devices::dispatch::{use_coldcard_simulator_for, use_ledger_simulator_for};
use crate::devices::ledger::{do_getxpub, open_ledger_by_fingerprint};
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
    if use_ledger_simulator_for(fingerprint).await? {
        let device = LedgerSimulator::try_connect()
            .await
            .map_err(|e| format!("speculos connect: {e:?}"))?
            .display_xpub(true)
            .map_err(|e| format!("display_xpub: {e:?}"))?;
        return do_getxpub(&device, path).await;
    }
    if use_coldcard_simulator_for(fingerprint)? {
        let (mut cc, _) = open_cc_simulator()?;
        return cc_getxpub(&mut cc, path);
    }
    let mut api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    if let Ok(mut cc) = open_coldcard_by_fingerprint(&mut api, fingerprint) {
        return cc_getxpub(&mut cc, path);
    }
    let device = open_ledger_by_fingerprint(&api, fingerprint)
        .await?
        .display_xpub(true)
        .map_err(|e| format!("display_xpub: {e:?}"))?;
    do_getxpub(&device, path).await
}
