//! `signtx` — sign a base64 PSBT on the device, returning the merged PSBT.

use async_hwi::ledger::{HidApi, LedgerSimulator};
use bitcoin::bip32::Fingerprint;

use crate::cli::Chain;
use crate::devices::coldcard::{
    do_signtx as cc_signtx, open_coldcard_by_fingerprint, open_simulator as open_cc_simulator,
    use_simulator as use_cc_simulator,
};
use crate::devices::ledger::{
    do_signtx, do_signtx_policy, open_ledger_by_fingerprint, use_simulator,
};
use crate::devices::mock::MockDevice;

/// What kind of signing request the CLI front-end produced.
///
/// `Default` is the existing single-sig path: build a default Ledger
/// wallet policy on the fly from the PSBT's BIP32 derivations.
/// `Policy` mirrors HWI PR #794 — the caller supplies the previously
/// registered BIP388 wallet policy (template + keys + hmac), and the
/// device drives one round of MuSig2 signing (round 1 yields pub
/// nonces, round 2 yields partial signatures). The Ledger app picks
/// the round based on what is already in the PSBT.
pub enum SignTxReq {
    Default {
        psbt: String,
    },
    Policy {
        psbt: String,
        name: String,
        template: String,
        keys: Vec<String>,
        hmac: String,
    },
}

pub async fn run_signtx(
    fingerprint: Fingerprint,
    chain: Chain,
    req: SignTxReq,
) -> Result<String, String> {
    if let Some(mock) = MockDevice::from_env() {
        // The mock has no notion of registered policies, so it just
        // signs with whatever derivations are already in the PSBT.
        // Good enough for non-MuSig2 round-trip smoke tests.
        let psbt = match &req {
            SignTxReq::Default { psbt } | SignTxReq::Policy { psbt, .. } => psbt.as_str(),
        };
        return mock.signtx(fingerprint, chain, psbt);
    }
    if use_simulator() {
        let device = LedgerSimulator::try_connect()
            .await
            .map_err(|e| format!("speculos connect: {e:?}"))?;
        return match req {
            SignTxReq::Default { psbt } => do_signtx(device, fingerprint, chain, &psbt).await,
            SignTxReq::Policy { .. } => do_signtx_policy(device, req).await,
        };
    }
    if use_cc_simulator() {
        let (mut cc, _) = open_cc_simulator()?;
        return match req {
            SignTxReq::Default { psbt } => cc_signtx(&mut cc, fingerprint, chain, &psbt),
            SignTxReq::Policy { .. } => {
                Err("policy-mode signtx is not yet supported for Coldcard".to_string())
            }
        };
    }
    let mut api = HidApi::new().map_err(|e| format!("hidapi init: {e}"))?;
    if let Ok(mut cc) = open_coldcard_by_fingerprint(&mut api, fingerprint) {
        return match req {
            SignTxReq::Default { psbt } => cc_signtx(&mut cc, fingerprint, chain, &psbt),
            SignTxReq::Policy { .. } => {
                Err("policy-mode signtx is not yet supported for Coldcard".to_string())
            }
        };
    }
    let device = open_ledger_by_fingerprint(&api, fingerprint).await?;
    match req {
        SignTxReq::Default { psbt } => do_signtx(device, fingerprint, chain, &psbt).await,
        SignTxReq::Policy { .. } => do_signtx_policy(device, req).await,
    }
}
