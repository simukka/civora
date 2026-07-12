use std::fmt;

use sha2::{Digest, Sha256};

/// Content id: the raw SHA-256 digest of the addressed bytes.
///
/// The digest is the canonical form in every wire and persisted encoding — a
/// `Cid` is 32 bytes on the wire, always. FNV (used for world content hashes in
/// `civora-sim`) is not collision-resistant, and content ids address
/// adversary-chosen bytes, so they need a cryptographic hash.
///
/// [`Cid::to_cid_string`] renders the standard CIDv1 text form for humans (the
/// patch-pack UI surface); it wraps the same digest without rehashing and is
/// presentation-only — nothing on the wire ever carries the text form.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Cid(pub [u8; 32]);

/// CIDv1 multihash header prefixed to the digest before base32 encoding:
/// version 1 (0x01) || raw codec (0x55) || sha2-256 (0x12) || digest len (0x20).
pub const CIDV1_RAW_SHA256_PREFIX: [u8; 4] = [0x01, 0x55, 0x12, 0x20];

/// Length of a rendered CIDv1 string: the `'b'` multibase prefix plus the
/// base32 encoding of 36 bytes (`ceil(36 * 8 / 5) = 58` symbols) = 59.
pub const CID_STRING_LEN: usize = 59;

/// RFC 4648 base32 lowercase alphabet, no padding (multibase `b`).
const BASE32_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

impl Cid {
    /// Content id of `content`: its SHA-256 digest.
    pub fn of(content: &[u8]) -> Cid {
        Cid(Sha256::digest(content).into())
    }

    /// Short display form (first 8 hex chars) for the HUD and logs.
    pub fn short(&self) -> String {
        self.0[..4].iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Render the standard CIDv1 text form: `'b'` (base32 multibase) followed by
    /// the base32-lowercase, unpadded encoding of
    /// `CIDV1_RAW_SHA256_PREFIX || digest`. Always [`CID_STRING_LEN`] chars.
    pub fn to_cid_string(&self) -> String {
        let mut bytes = Vec::with_capacity(CIDV1_RAW_SHA256_PREFIX.len() + self.0.len());
        bytes.extend_from_slice(&CIDV1_RAW_SHA256_PREFIX);
        bytes.extend_from_slice(&self.0);
        let mut s = String::with_capacity(CID_STRING_LEN);
        s.push('b');
        base32_encode(&bytes, &mut s);
        s
    }

    /// Parse a CIDv1 text form, the strict inverse of [`Cid::to_cid_string`].
    ///
    /// Rejects, in order: length not [`CID_STRING_LEN`]; a leading char other
    /// than `'b'`; any non-alphabet symbol (uppercase, `=`, `0/1/8/9`); non-zero
    /// trailing padding bits; a header that is not a raw sha2-256 CIDv1.
    pub fn from_cid_string(s: &str) -> Option<Cid> {
        if s.len() != CID_STRING_LEN {
            return None;
        }
        let rest = s.strip_prefix('b')?;
        let bytes = base32_decode(rest)?;
        let (header, digest) = bytes.split_at(CIDV1_RAW_SHA256_PREFIX.len());
        if header != CIDV1_RAW_SHA256_PREFIX {
            return None;
        }
        Some(Cid(digest.try_into().expect("36 - 4 = 32 byte digest")))
    }
}

/// Append the base32-lowercase, unpadded encoding of `data` to `out`.
fn base32_encode(data: &[u8], out: &mut String) {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in data {
        acc = (acc << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32_ALPHABET[((acc >> bits) & 0x1f) as usize] as char);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        // Pad the final symbol's low bits with zeros.
        out.push(BASE32_ALPHABET[((acc << (5 - bits)) & 0x1f) as usize] as char);
    }
}

/// Decode a base32-lowercase, unpadded string. Returns `None` on any symbol
/// outside the alphabet or non-zero trailing padding bits.
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    for byte in s.bytes() {
        let value = match byte {
            b'a'..=b'z' => byte - b'a',
            b'2'..=b'7' => byte - b'2' + 26,
            _ => return None,
        };
        acc = (acc << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // The bits left over after the last whole byte are padding and must be zero.
    (acc == 0).then_some(out)
}

impl fmt::Display for Cid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The well-known IPFS CIDv1 of an empty raw block — an external cross-check
    /// that the encoding matches the ecosystem, not just itself.
    const EMPTY_CID: &str = "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku";
    /// A stability pin: `Cid::of(b"civora")` must never silently change.
    const CIVORA_CID: &str = "bafkreiauohq3h7rrn2a5vtmxenkiz4gdjiyvy3w25t4crlcrdohjdme44m";

    #[test]
    fn golden_vectors() {
        assert_eq!(Cid::of(b"").to_cid_string(), EMPTY_CID);
        assert_eq!(Cid::of(b"civora").to_cid_string(), CIVORA_CID);
        assert_eq!(EMPTY_CID.len(), CID_STRING_LEN);
        assert_eq!(CIVORA_CID.len(), CID_STRING_LEN);
    }

    #[test]
    fn round_trips() {
        for content in [&b""[..], b"civora", b"the quick brown fox", &[0xff; 100]] {
            let cid = Cid::of(content);
            let text = cid.to_cid_string();
            assert_eq!(Cid::from_cid_string(&text), Some(cid));
        }
        // Every-byte digest round-trips too (exercises the full symbol table).
        let cid = Cid((0..32).collect::<Vec<u8>>().try_into().unwrap());
        assert_eq!(Cid::from_cid_string(&cid.to_cid_string()), Some(cid));
    }

    #[test]
    fn from_cid_string_rejects_malformed() {
        let valid = Cid::of(b"civora").to_cid_string();

        // Wrong length.
        assert_eq!(Cid::from_cid_string(&valid[..58]), None);
        assert_eq!(Cid::from_cid_string(&format!("{valid}a")), None);

        // Wrong multibase prefix (swap leading 'b' for another alphabet char,
        // keeping the length; decodes but the header no longer matches).
        let mut wrong_prefix = valid.clone();
        wrong_prefix.replace_range(0..1, "c");
        assert_eq!(Cid::from_cid_string(&wrong_prefix), None);

        // Non-alphabet symbols: uppercase, padding, and the excluded digits.
        for bad in ['A', '=', '0', '1', '8', '9'] {
            let mut s = valid.clone();
            s.replace_range(1..2, &bad.to_string());
            assert_eq!(Cid::from_cid_string(&s), None, "char {bad:?}");
        }

        // Non-zero trailing bits: the final symbol carries 2 padding bits that
        // must be zero. 'b' (00001) sets a padding bit, so it is never a valid
        // last symbol whatever the original was.
        let mut trailing = valid.clone();
        trailing.replace_range(58..59, "b");
        assert_eq!(Cid::from_cid_string(&trailing), None);
    }
}
