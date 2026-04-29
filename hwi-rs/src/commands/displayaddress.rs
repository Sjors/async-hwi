//! `displayaddress` — show an address on the device for confirmation.

use async_hwi::ledger::{HidApi, LedgerSimulator};
use bitcoin::bip32::Fingerprint;

use crate::cli::Chain;
use crate::devices::coldcard::{
    do_displayaddress as cc_displayaddress, open_coldcard_by_fingerprint,
    open_simulator as open_cc_simulator, use_simulator as use_cc_simulator,
};
use crate::devices::ledger::{
    do_displayaddress, do_displayaddress_policy, open_ledger_by_fingerprint, use_simulator,
};
use crate::devices::mock::MockDevice;

/// What kind of address-display request the CLI front-end produced.
///
/// `SingleSig` is the Bitcoin Core path (just `--desc <definite-descriptor>`).
/// `Policy` mirrors HWI PR #794 — the caller supplies the registered
/// BIP388 wallet policy, the hmac it was registered with, and the
/// (change, index) of the address to derive.
pub enum DisplayAddressReq {
    SingleSig {
        desc: String,
    },
    Policy {
        name: String,
        template: String,
        keys: Vec<String>,
        hmac: String,
        index: u32,
        change: bool,
    },
}

pub async fn run_displayaddress(
    fingerprint: Fingerprint,
    chain: Chain,
    req: DisplayAddressReq,
) -> Result<String, String> {
    if let Some(mock) = MockDevice::from_env() {
        return match req {
            DisplayAddressReq::SingleSig { desc } => mock.displayaddress(fingerprint, chain, &desc),
            DisplayAddressReq::Policy { .. } => mock.displayaddress_policy(fingerprint, chain, req),
        };
    }
    if use_simulator() {
        let device = LedgerSimulator::try_connect()
            .await
            .map_err(|e| format!("speculos connect: {e:?}"))?;
        return match req {
            DisplayAddressReq::SingleSig { desc } => {
                do_displayaddress(device, fingerprint, chain, &desc).await
            }
            DisplayAddressReq::Policy { .. } => do_displayaddress_policy(device, chain, req).await,
        };
    }
    if use_cc_simulator() {
        let (mut cc, _) = open_cc_simulator()?;
        return match req {
            DisplayAddressReq::SingleSig { desc } => cc_displayaddress(&mut cc, chain, &desc),
            DisplayAddressReq::Policy { .. } => {
                return Err("displayaddress --policy-name is not yet supported for Coldcard".into())
            }
        };
    }
    let mut api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    if let Ok(mut cc) = open_coldcard_by_fingerprint(&mut api, fingerprint) {
        return match req {
            DisplayAddressReq::SingleSig { desc } => cc_displayaddress(&mut cc, chain, &desc),
            DisplayAddressReq::Policy { .. } => {
                return Err("displayaddress --policy-name is not yet supported for Coldcard".into())
            }
        };
    }
    let device = open_ledger_by_fingerprint(&api, fingerprint).await?;
    match req {
        DisplayAddressReq::SingleSig { desc } => {
            do_displayaddress(device, fingerprint, chain, &desc).await
        }
        DisplayAddressReq::Policy { .. } => do_displayaddress_policy(device, chain, req).await,
    }
}
