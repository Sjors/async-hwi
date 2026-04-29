//! Backend selection for per-fingerprint subcommands.
//!
//! Bitcoin Core can be configured to talk to more than one signer at a
//! time (the kumbaya 3-of-3 MuSig2 scenario combines a Ledger speculos,
//! a Coldcard simulator, and a Core-hot key). When that happens the
//! `enumerate` subcommand reports both devices, and Core then dispatches
//! follow-up subcommands by passing `--fingerprint`. Each per-fingerprint
//! command must therefore decide which backend (Ledger sim, Coldcard sim,
//! or HID) to drive.
//!
//! When only one of the simulator env vars is set, behavior is unchanged.

use async_hwi::ledger::LedgerSimulator;
use bitcoin::bip32::Fingerprint;

use crate::devices::coldcard::{open_simulator as open_cc_simulator, use_simulator as use_cc_sim};
use crate::devices::ledger::use_simulator as use_ledger_sim;

/// True iff the Ledger speculos should be used for the request targeting
/// `want`. When both simulator env vars are set, the device's master
/// fingerprint is probed and matched against `want`.
pub async fn use_ledger_simulator_for(want: Fingerprint) -> Result<bool, String> {
    if !use_ledger_sim() {
        return Ok(false);
    }
    if !use_cc_sim() {
        return Ok(true);
    }
    Ok(ledger_simulator_fingerprint().await? == want)
}

/// True iff the Coldcard simulator should be used for the request
/// targeting `want`. When both simulator env vars are set, the device's
/// master fingerprint is probed and matched against `want`.
pub fn use_coldcard_simulator_for(want: Fingerprint) -> Result<bool, String> {
    if !use_cc_sim() {
        return Ok(false);
    }
    if !use_ledger_sim() {
        return Ok(true);
    }
    let (_cc, fp) = open_cc_simulator()?;
    Ok(fp == want)
}

async fn ledger_simulator_fingerprint() -> Result<Fingerprint, String> {
    let device = LedgerSimulator::try_connect()
        .await
        .map_err(|e| format!("speculos connect: {e:?}"))?;
    async_hwi::HWI::get_master_fingerprint(&device)
        .await
        .map_err(|e| format!("get_master_fingerprint: {e:?}"))
}
