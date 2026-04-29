//! In-process software signer used by CI and local testing.
//!
//! Backed by [BIP32 test vector 1](https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki#test-vector-1)
//! (seed `000102030405060708090a0b0c0d0e0f`, master fingerprint `3442193e`),
//! so future `signtx` support can produce real signatures. Embedding the
//! corresponding xprivs is safe — they are published in BIP32 itself and
//! well known to every implementation. The mock only ever derives keys at
//! non-mainnet coin types from this seed, so accidental real-world use is
//! hard to engineer. Enabled with `HWI_RS_MOCK=1`.
//!
//! The mock is device-agnostic: it always reports `type:"mock"` /
//! `model:"mock"` regardless of which physical signer it is standing in
//! for. Per-device CI matrix entries differ only in the integration
//! script that drives them, not in what the mock claims to be.

use bitcoin::bip32::{Fingerprint, Xpriv};
use bitcoin::secp256k1::Secp256k1;
use bitcoin::Network;

use crate::devices::DeviceEntry;

const MOCK_SEED: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];

pub struct MockDevice {
    secp: Secp256k1<bitcoin::secp256k1::All>,
}

impl MockDevice {
    pub fn from_env() -> Option<Self> {
        if std::env::var("HWI_RS_MOCK").ok().as_deref() != Some("1") {
            return None;
        }
        Some(MockDevice {
            secp: Secp256k1::new(),
        })
    }

    fn master(&self) -> Xpriv {
        Xpriv::new_master(Network::Bitcoin, &MOCK_SEED).expect("BIP32 master from fixed seed")
    }

    pub fn fingerprint(&self) -> Fingerprint {
        // Network does not affect the master fingerprint (hash160 of pubkey).
        self.master().fingerprint(&self.secp)
    }

    pub fn enumerate(&self) -> Result<String, String> {
        let entry = DeviceEntry {
            kind: "mock",
            model: "mock".to_string(),
            label: None,
            path: "mock".to_string(),
            fingerprint: Some(format!("{:x}", self.fingerprint())),
            needs_pin_sent: false,
            needs_passphrase_sent: false,
            error: None,
        };
        serde_json::to_string(&[entry]).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock() -> MockDevice {
        MockDevice {
            secp: Secp256k1::new(),
        }
    }

    #[test]
    fn mock_fingerprint_matches_bip32_vector_1() {
        // BIP32 test vector 1 master fingerprint.
        assert_eq!(format!("{:x}", mock().fingerprint()), "3442193e");
    }
}
