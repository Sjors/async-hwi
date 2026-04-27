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

use bitcoin::bip32::{DerivationPath, Fingerprint, Xpriv, Xpub};
use bitcoin::secp256k1::Secp256k1;

use crate::cli::Chain;
use crate::commands::GetDescriptorsOut;
use crate::descriptor::{address_from_descriptor, format_descriptor, ADDR_TYPES};
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

    fn master(&self, chain: Chain) -> Xpriv {
        Xpriv::new_master(chain.network(), &MOCK_SEED).expect("BIP32 master from fixed seed")
    }

    pub fn fingerprint(&self) -> Fingerprint {
        // Network does not affect the master fingerprint (hash160 of pubkey).
        self.master(Chain::Main).fingerprint(&self.secp)
    }

    fn require_fingerprint(&self, want: Fingerprint) -> Result<(), String> {
        let have = self.fingerprint();
        if have != want {
            return Err(format!(
                "no mock device matching fingerprint {want:x} (have {have:x})"
            ));
        }
        Ok(())
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

    pub fn getdescriptors(
        &self,
        fingerprint: Fingerprint,
        chain: Chain,
        account: u32,
    ) -> Result<String, String> {
        self.require_fingerprint(fingerprint)?;
        let master = self.master(chain);
        let coin = chain.coin_type();
        let mut receive = Vec::new();
        let mut internal = Vec::new();
        for &(purpose, wrapper) in ADDR_TYPES {
            let path: DerivationPath = format!("m/{purpose}h/{coin}h/{account}h")
                .parse()
                .map_err(|e| format!("path parse: {e}"))?;
            let xpriv = master
                .derive_priv(&self.secp, &path)
                .map_err(|e| format!("derive: {e}"))?;
            let xpub = Xpub::from_priv(&self.secp, &xpriv);
            let origin = format!("[{fingerprint:x}/{purpose}h/{coin}h/{account}h]");
            receive.push(format_descriptor(wrapper, &origin, &xpub.to_string(), 0));
            internal.push(format_descriptor(wrapper, &origin, &xpub.to_string(), 1));
        }
        let out = GetDescriptorsOut { receive, internal };
        serde_json::to_string(&out).map_err(|e| e.to_string())
    }

    pub fn displayaddress(
        &self,
        fingerprint: Fingerprint,
        chain: Chain,
        desc: &str,
    ) -> Result<String, String> {
        self.require_fingerprint(fingerprint)?;
        let address = address_from_descriptor(desc, chain)?;
        Ok(serde_json::json!({ "address": address }).to_string())
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

    #[test]
    fn mock_descriptors_use_requested_fingerprint() {
        let mock = mock();
        let fp = mock.fingerprint();
        let json = mock.getdescriptors(fp, Chain::Regtest, 0).unwrap();
        let parsed: GetDescriptorsOut = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.receive.len(), 4);
        assert_eq!(parsed.internal.len(), 4);
        let prefix_h = format!("pkh([{fp:x}/44h/1h/0h]");
        let prefix_q = format!("pkh([{fp:x}/44'/1'/0']");
        assert!(
            parsed.receive[0].starts_with(&prefix_h) || parsed.receive[0].starts_with(&prefix_q),
            "got {}",
            parsed.receive[0]
        );
        for d in parsed.receive.iter().chain(parsed.internal.iter()) {
            assert!(d.contains('#'), "missing checksum: {}", d);
        }
    }

    #[test]
    fn mock_displayaddress_echoes_descriptor_address() {
        let mock = mock();
        let fp = mock.fingerprint();
        let desc = "wpkh([00000001/84h/0h/0h/0/0]025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee6357)";
        let json = mock.displayaddress(fp, Chain::Main, desc).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v["address"].as_str().unwrap(),
            "bc1qr583w2swedy2acd7rung055k8t3n7udp7vyzyg"
        );
    }
}
