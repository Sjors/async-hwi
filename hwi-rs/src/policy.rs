//! Mapping inferred descriptors / PSBT BIP32 paths to the four default
//! Ledger Bitcoin app wallet policies (`pkh(@0/**)`, `sh(wpkh(@0/**))`,
//! `wpkh(@0/**)`, `tr(@0/**)`).
//!
//! These four templates are recognised by the new Ledger Bitcoin app
//! without any prior on-device wallet registration: passing them to
//! `get_wallet_address` / `sign_psbt` with `wallet_hmac=None` works, as
//! long as the BIP32 derivation matches `m/{purpose}'/{coin}'/{account}'/
//! {change}/{index}` with the conventional purpose for the script type.
//! Mirrors what Python HWI does in `_get_singlesig_default_wallet_policy`.

use bitcoin::bip32::{ChildNumber, Fingerprint, Xpub};
use miniscript::{descriptor::DescriptorType, Descriptor, DescriptorPublicKey};

use crate::descriptor::single_key_full_path;

/// Build the default policy string `wpkh([fp/84h/.../acct']xpub/**)` (or
/// equivalent for the other three flavours), in the form `async-hwi`'s
/// [`Ledger::with_wallet`] expects.
pub fn build_default_policy(
    purpose: u32,
    fingerprint: Fingerprint,
    coin: u32,
    account: u32,
    xpub: &Xpub,
) -> String {
    let key = format!("[{fingerprint:x}/{purpose}h/{coin}h/{account}h]{xpub}");
    match purpose {
        44 => format!("pkh({key}/**)"),
        49 => format!("sh(wpkh({key}/**))"),
        84 => format!("wpkh({key}/**)"),
        86 => format!("tr({key}/**)"),
        // Caller is expected to validate purpose before constructing a policy.
        other => panic!("hwi-rs: no default Ledger wallet policy for BIP{}", other),
    }
}

/// One inferred single-sig output, matched against a default Ledger
/// Bitcoin app wallet policy.
pub struct SingleSig {
    /// BIP44 purpose: 44 (pkh) / 49 (sh-wpkh) / 84 (wpkh) / 86 (tr).
    pub purpose: u32,
    pub account: u32,
    /// `false` = receive (chain 0), `true` = change (chain 1).
    pub change: bool,
    pub index: u32,
}

/// Match an inferred (definite) descriptor to a default single-sig wallet
/// policy and BIP44 path components. Used by `displayaddress`.
///
/// Rejects multisig, miniscript, and any path that is not the canonical
/// `m/{purpose}'/{coin}'/{account}'/{change}/{index}` BIP44 layout. Taproot
/// script-tree spends are not detected here — the device will reject them
/// when the policy template `tr(@0/**)` does not match.
pub fn classify_singlesig(
    desc: &Descriptor<DescriptorPublicKey>,
    coin: u32,
) -> Result<SingleSig, String> {
    let purpose = match desc.desc_type() {
        DescriptorType::Pkh => 44,
        DescriptorType::ShWpkh => 49,
        DescriptorType::Wpkh => 84,
        DescriptorType::Tr => 86,
        other => {
            return Err(format!(
                "descriptor type {other:?} not supported by hwi-rs displayaddress \
                 (only single-sig BIP44/49/84/86)"
            ))
        }
    };
    let path = single_key_full_path(desc)
        .ok_or_else(|| "descriptor key has no BIP32 origin".to_string())?;
    let children: Vec<ChildNumber> = path.into();
    if children.len() != 5 {
        return Err(format!(
            "expected 5-component BIP44 path (m/p'/c'/a'/change/index), got {} components",
            children.len()
        ));
    }
    let (p, c, a, change, index) = match (
        &children[0],
        &children[1],
        &children[2],
        &children[3],
        &children[4],
    ) {
        (
            ChildNumber::Hardened { index: p },
            ChildNumber::Hardened { index: c },
            ChildNumber::Hardened { index: a },
            ChildNumber::Normal { index: ch },
            ChildNumber::Normal { index: i },
        ) => (*p, *c, *a, *ch, *i),
        _ => {
            return Err("BIP44 path must be m/p'/c'/a'/change/index (3 hardened, 2 normal)".into())
        }
    };
    if p != purpose {
        return Err(format!(
            "descriptor wrapper implies BIP{purpose} but path has purpose {p}"
        ));
    }
    if c != coin {
        return Err(format!(
            "coin type {c} in path does not match active chain (coin {coin})"
        ));
    }
    if change > 1 {
        return Err(format!(
            "change index {change} is neither receive (0) nor change (1)"
        ));
    }
    Ok(SingleSig {
        purpose,
        account: a,
        change: change == 1,
        index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn classify_singlesig_accepts_standard_wpkh() {
        let d: Descriptor<DescriptorPublicKey> = Descriptor::from_str(
            "wpkh([d34db33f/84h/0h/0h/0/7]025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee6357)",
        )
        .unwrap();
        let s = classify_singlesig(&d, 0).unwrap();
        assert_eq!(s.purpose, 84);
        assert_eq!(s.account, 0);
        assert!(!s.change);
        assert_eq!(s.index, 7);
    }

    #[test]
    fn classify_singlesig_rejects_purpose_mismatch() {
        // wpkh wrapper but path uses BIP44 purpose.
        let d: Descriptor<DescriptorPublicKey> = Descriptor::from_str(
            "wpkh([d34db33f/44h/0h/0h/0/0]025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee6357)",
        )
        .unwrap();
        assert!(classify_singlesig(&d, 0).is_err());
    }

    #[test]
    fn classify_singlesig_rejects_short_path() {
        let d: Descriptor<DescriptorPublicKey> = Descriptor::from_str(
            "wpkh([d34db33f/84h/0h/0h]025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee6357)",
        )
        .unwrap();
        assert!(classify_singlesig(&d, 0).is_err());
    }
}
