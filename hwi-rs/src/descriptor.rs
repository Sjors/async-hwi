//! Descriptor helpers: building checksummed descriptors.
//!
//! BIP380 checksumming is delegated to `rust-miniscript`'s `Display` impl —
//! parsing a body and re-emitting it appends `#<checksum>` automatically.

use std::str::FromStr;

use miniscript::{Descriptor, DescriptorPublicKey};

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
}
