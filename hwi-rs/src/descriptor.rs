//! Descriptor helpers: building checksummed descriptors and inspecting the
//! definite descriptors Bitcoin Core hands us via `walletdisplayaddress`.
//!
//! BIP380 checksumming is delegated to `rust-miniscript`'s `Display` impl —
//! parsing a body and re-emitting it appends `#<checksum>` automatically.

use std::str::FromStr;

use bitcoin::bip32::DerivationPath;
use miniscript::{Descriptor, DescriptorPublicKey, ForEachKey};

use crate::cli::Chain;

/// (BIP44 purpose, descriptor wrapper applied to `KEY/RANGE/*`). The four
/// default Ledger Bitcoin app single-sig wallet policies.
pub const ADDR_TYPES: &[(u32, &str)] = &[(44, "pkh"), (49, "sh-wpkh"), (84, "wpkh"), (86, "tr")];

/// Build a checksummed descriptor of one of the four default single-sig
/// shapes, given the `[fp/...]` origin string, the account-level xpub, and
/// the change branch (0 receive / 1 change).
pub fn format_descriptor(wrapper: &str, origin: &str, xpub: &str, branch: u32) -> String {
    let inner_key = format!("{origin}{xpub}/{branch}/*");
    let body = match wrapper {
        "sh-wpkh" => format!("sh(wpkh({inner_key}))"),
        other => format!("{other}({inner_key})"),
    };
    // Round-tripping through miniscript appends the BIP380 checksum and
    // normalises the formatting (e.g. uses `h` for hardened markers).
    Descriptor::<DescriptorPublicKey>::from_str(&body)
        .expect("hwi-rs constructs a valid descriptor body")
        .to_string()
}

/// Compute the address encoded by a definite (no-wildcard) descriptor.
///
/// Bitcoin Core's external_signer_scriptpubkeyman feeds us the output of
/// `InferDescriptor(scriptPubKey, provider)`: a definite, single-key
/// descriptor with full BIP32 origin path but no wildcards. Core rejects
/// the call if the echoed string does not match the address it passed.
pub fn address_from_descriptor(desc: &str, chain: Chain) -> Result<String, String> {
    let parsed: Descriptor<DescriptorPublicKey> =
        desc.parse().map_err(|e| format!("descriptor parse: {e}"))?;
    if parsed.has_wildcard() {
        return Err("descriptor has wildcards; expected a definite descriptor".into());
    }
    let definite = parsed
        .at_derivation_index(0)
        .map_err(|e| format!("descriptor derive: {e}"))?;
    definite
        .address(chain.network())
        .map(|a| a.to_string())
        .map_err(|e| format!("descriptor address: {e}"))
}

/// Return the full BIP32 derivation path (origin + any final children) of
/// the single key in a definite descriptor.
pub fn single_key_full_path(desc: &Descriptor<DescriptorPublicKey>) -> Option<DerivationPath> {
    let mut path: Option<DerivationPath> = None;
    desc.for_each_key(|k| {
        path = match k {
            DescriptorPublicKey::Single(s) => s.origin.as_ref().map(|(_, p)| p.clone()),
            DescriptorPublicKey::XPub(x) => {
                let base = x
                    .origin
                    .as_ref()
                    .map(|(_, p)| p.clone())
                    .unwrap_or_else(DerivationPath::master);
                Some(base.extend(&x.derivation_path))
            }
            _ => None,
        };
        true
    });
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip a known descriptor through `format_descriptor`. The
    /// result must parse back to the same definite descriptor (modulo
    /// `h` vs `'` normalisation that miniscript performs) and end in a
    /// BIP380 `#<checksum>` suffix. We check the suffix exists and is
    /// 8 chars from the bech32 charset rather than pinning an exact
    /// value, since the canonical form depends on miniscript's chosen
    /// hardened marker.
    #[test]
    fn format_descriptor_appends_checksum() {
        let s = format_descriptor(
            "wpkh",
            "[d34db33f/84h/0h/0h]",
            "xpub6DJ2dNUysrn5Vt36jH2KLBT2i1auw1tTSSomg8PhqNiUtx8QX2SvC9nrHu81fT41fvDUnhMjEzQgXnQjKEu3oaqMSzhSrHMxyyoEAmUHQbY",
            0,
        );
        let (body, cs) = s
            .rsplit_once('#')
            .unwrap_or_else(|| panic!("no checksum: {}", s));
        assert_eq!(cs.len(), 8, "checksum must be 8 chars: {s}");
        assert!(cs
            .bytes()
            .all(|b| b"qpzry9x8gf2tvdw0s3jn54khce6mua7l".contains(&b)));
        // Round-trip back through miniscript: the canonical form must equal `s`.
        let canon = Descriptor::<DescriptorPublicKey>::from_str(body)
            .unwrap()
            .to_string();
        assert_eq!(canon, s);
    }

    #[test]
    fn address_from_inferred_wpkh_descriptor() {
        // A definite (no wildcard) wpkh descriptor with a raw pubkey, the
        // shape produced by Bitcoin Core's `InferDescriptor` for a single
        // scriptPubKey. Compressed pubkey from BIP143 test vectors.
        let desc = "wpkh([d34db33f/84h/0h/0h/0/0]025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee6357)";
        let addr = address_from_descriptor(desc, Chain::Main).unwrap();
        assert_eq!(addr, "bc1qr583w2swedy2acd7rung055k8t3n7udp7vyzyg");
    }
}
