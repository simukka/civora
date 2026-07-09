use std::fmt;

use sha2::{Digest, Sha256};

/// Placeholder content id: the raw SHA-256 digest of the addressed bytes.
///
/// The patch-pack milestone wraps these digests into real CIDv1s without
/// rehashing; until then a `Cid` is just the digest. FNV (used for world
/// content hashes in `civora-sim`) is not collision-resistant, and content
/// ids address adversary-chosen bytes, so they need a cryptographic hash.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Cid(pub [u8; 32]);

impl Cid {
    /// Content id of `content`: its SHA-256 digest.
    pub fn of(content: &[u8]) -> Cid {
        Cid(Sha256::digest(content).into())
    }

    /// Short display form (first 8 hex chars) for the HUD and logs.
    pub fn short(&self) -> String {
        self.0[..4].iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Display for Cid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}
