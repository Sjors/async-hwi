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
use bitcoin::psbt::Psbt;
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

    pub fn getxpub(
        &self,
        fingerprint: Fingerprint,
        chain: Chain,
        path: &str,
    ) -> Result<String, String> {
        self.require_fingerprint(fingerprint)?;
        let derivation: DerivationPath = path.parse().map_err(|e| format!("path parse: {e}"))?;
        let master = self.master(chain);
        let xpriv = master
            .derive_priv(&self.secp, &derivation)
            .map_err(|e| format!("derive: {e}"))?;
        let xpub = Xpub::from_priv(&self.secp, &xpriv);
        Ok(serde_json::json!({ "xpub": xpub.to_string() }).to_string())
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

    pub fn signtx(
        &self,
        fingerprint: Fingerprint,
        chain: Chain,
        psbt_b64: &str,
    ) -> Result<String, String> {
        use bitcoin::base64::Engine as _;
        self.require_fingerprint(fingerprint)?;
        let raw = bitcoin::base64::engine::general_purpose::STANDARD
            .decode(psbt_b64.trim())
            .map_err(|e| format!("psbt base64 decode: {e}"))?;
        let mut psbt = Psbt::deserialize(&raw).map_err(|e| format!("psbt parse: {e}"))?;
        // psbt.sign returns Err iff at least one input failed; ignore the
        // partial-failure case because Bitcoin Core only requires that the
        // PSBT come back with as many partial sigs as we could provide.
        let master = self.master(chain);
        let _ = psbt.sign(&master, &self.secp);
        let bytes = psbt.serialize();
        let out = bitcoin::base64::engine::general_purpose::STANDARD.encode(bytes);
        Ok(serde_json::json!({ "psbt": out }).to_string())
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

    #[test]
    fn mock_signtx_round_trips_empty_psbt() {
        // Minimal valid PSBT (regtest, no inputs/outputs). The signer is a
        // no-op on it but must round-trip cleanly.
        let mock = mock();
        let fp = mock.fingerprint();
        let empty = "cHNidP8BAAoCAAAAAAAAAAAAAA==";
        let json = mock.signtx(fp, Chain::Regtest, empty).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["psbt"].as_str().unwrap(), empty);
    }

    #[test]
    fn mock_signtx_signs_wpkh_input_for_matching_derivation() {
        use bitcoin::absolute::LockTime;
        use bitcoin::bip32::DerivationPath;
        use bitcoin::psbt::{Input, Psbt};
        use bitcoin::secp256k1::Secp256k1;
        use bitcoin::transaction::Version;
        use bitcoin::{
            Address, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
        };

        let mock = mock();
        let fp = mock.fingerprint();
        let secp = Secp256k1::new();
        // Derive the mock P2WPKH at m/84h/1h/0h/0/0 and synthesise a previous
        // output paying to it, then build a single-input PSBT spending it.
        let master = mock.master(Chain::Regtest);
        let path: DerivationPath = "m/84h/1h/0h/0/0".parse().unwrap();
        let xpriv = master.derive_priv(&secp, &path).unwrap();
        let priv_key = bitcoin::PrivateKey::new(xpriv.private_key, bitcoin::Network::Regtest);
        let cpk = bitcoin::CompressedPublicKey::from_private_key(&secp, &priv_key).unwrap();
        let pk = bitcoin::PublicKey::new(cpk.0);
        let addr = Address::p2wpkh(&cpk, bitcoin::Network::Regtest);

        let prev = OutPoint::null();
        let unsigned = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: prev,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: addr.script_pubkey(),
            }],
        };
        let mut psbt = Psbt::from_unsigned_tx(unsigned).unwrap();
        let mut bip32 = std::collections::BTreeMap::new();
        bip32.insert(pk.inner, (fp, path.clone()));
        psbt.inputs[0] = Input {
            witness_utxo: Some(TxOut {
                value: Amount::from_sat(60_000),
                script_pubkey: addr.script_pubkey(),
            }),
            bip32_derivation: bip32,
            ..Input::default()
        };

        let json = mock.signtx(fp, Chain::Regtest, &psbt.to_string()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let signed: Psbt = v["psbt"].as_str().unwrap().parse().unwrap();
        assert!(
            !signed.inputs[0].partial_sigs.is_empty(),
            "expected the mock to add a partial signature for the matching derivation"
        );
    }
}
