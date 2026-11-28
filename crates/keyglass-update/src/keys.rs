//! The public keys built into every host (`keys/` in this crate).
//!
//! See `keys/README.md` for what each key signs and how the keys are kept.

use crate::trust::{PublicKey, TrustAnchors};

/// The root public key file built into this crate.
pub const ROOT_PUB: &str = include_str!("../keys/root.pub");
/// The initial release public key file built into this crate.
pub const RELEASE_PUB: &str = include_str!("../keys/release.pub");

/// Base64 lines of placeholder public keys that must never be trust anchors:
/// no secret key exists for them, so a host that trusted them could never
/// verify an update. The `production-keys` feature refuses to embed them.
pub const DEVELOPMENT_KEYS: [&str; 2] = [
    "RWTM73331SFS//pr69lWKM9McvwTXXZk9XyF3T8/6hrRAZVZVgt7p5F0",
    "RWTwabwNcFsP2Ax7KGtbyx8XiqAsaIYrCl7NxoAVRdQl7rIAMLiMjrAv",
];

const fn contains(haystack: &str, needle: &str) -> bool {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.len() > h.len() {
        return false;
    }
    let mut i = 0;
    while i + n.len() <= h.len() {
        let mut j = 0;
        while j < n.len() && h[i + j] == n[j] {
            j += 1;
        }
        if j == n.len() {
            return true;
        }
        i += 1;
    }
    false
}

const fn holds_development_key(file: &str) -> bool {
    let mut i = 0;
    while i < DEVELOPMENT_KEYS.len() {
        if contains(file, DEVELOPMENT_KEYS[i]) {
            return true;
        }
        i += 1;
    }
    false
}

/// True when an embedded key is one of [`DEVELOPMENT_KEYS`].
pub const EMBEDS_DEVELOPMENT_KEYS: bool =
    holds_development_key(ROOT_PUB) || holds_development_key(RELEASE_PUB);

#[cfg(feature = "production-keys")]
const _: () = {
    if EMBEDS_DEVELOPMENT_KEYS {
        panic!("production-keys is enabled but keys/root.pub or keys/release.pub is a placeholder development key; see keys/README.md");
    }
};

/// The trust anchors every host is built with.
pub fn trust_anchors() -> TrustAnchors {
    TrustAnchors {
        root_keys: vec![PublicKey::parse(ROOT_PUB).expect("keys/root.pub is a minisign public key")],
        release_keys: vec![
            PublicKey::parse(RELEASE_PUB).expect("keys/release.pub is a minisign public key")
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_keys_parse_and_are_distinct() {
        let a = trust_anchors();
        assert_ne!(a.root_keys[0].id(), a.release_keys[0].id());
        assert!(!holds_development_key(ROOT_PUB) && !holds_development_key(RELEASE_PUB));
    }

    #[test]
    fn development_keys_are_recognised() {
        for key in DEVELOPMENT_KEYS {
            let file = format!("untrusted comment: development key\n{key}\n");
            assert!(holds_development_key(&file));
            assert!(PublicKey::parse(key).is_ok());
        }
        assert!(!holds_development_key(
            "untrusted comment: x\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n"
        ));
    }
}
