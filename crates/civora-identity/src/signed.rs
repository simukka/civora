use std::fmt;

use civora_sim::Action;
use ed25519_dalek::{Signature, VerifyingKey};

use crate::identity::PlayerId;

/// Domain-separation prefix for action signatures, so a signature over an
/// action can never be confused with one over a future message type
/// (votes, proposals, ...).
pub const ACTION_SIGN_DOMAIN: &[u8] = b"civora.action.v1";

/// An [`Action`] bound to its author and per-author sequence number by an
/// Ed25519 signature over the canonical payload
/// `domain || author || seq (u64 LE) || Action::encode`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SignedAction {
    pub author: PlayerId,
    pub seq: u64,
    pub action: Action,
    pub signature: [u8; 64],
}

pub(crate) fn signing_payload(author: &PlayerId, seq: u64, action: &Action) -> Vec<u8> {
    let mut payload = Vec::with_capacity(ACTION_SIGN_DOMAIN.len() + 32 + 8 + 14);
    payload.extend_from_slice(ACTION_SIGN_DOMAIN);
    payload.extend_from_slice(&author.0);
    payload.extend_from_slice(&seq.to_le_bytes());
    action.encode(&mut payload);
    payload
}

impl SignedAction {
    /// Append the canonical wire encoding of this signed action to `out`:
    /// `author (32) || seq (u64 LE) || action_len (u16 LE) || action bytes ||
    /// signature (64)`.
    ///
    /// The length prefix makes the encoding self-delimiting so lists of
    /// signed actions (gossip messages, transferred logs) decode by
    /// iteration.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.author.0);
        out.extend_from_slice(&self.seq.to_le_bytes());
        let mut action = Vec::with_capacity(14);
        self.action.encode(&mut action);
        out.extend_from_slice(&(action.len() as u16).to_le_bytes());
        out.extend_from_slice(&action);
        out.extend_from_slice(&self.signature);
    }

    /// Decode one signed action from the front of `bytes`, returning it and
    /// the remaining bytes.
    ///
    /// Returns `None` for truncated input or an inner action that
    /// [`Action::decode`] rejects. Decoding checks structure only — call
    /// [`SignedAction::verify`] (or append to [`crate::ActionLog`]) before
    /// trusting the result.
    pub fn decode(bytes: &[u8]) -> Option<(SignedAction, &[u8])> {
        fn take(bytes: &[u8], n: usize) -> Option<(&[u8], &[u8])> {
            (bytes.len() >= n).then(|| bytes.split_at(n))
        }

        let (author, rest) = take(bytes, 32)?;
        let (seq, rest) = take(rest, 8)?;
        let (len, rest) = take(rest, 2)?;
        let action_len = u16::from_le_bytes(len.try_into().unwrap()) as usize;
        let (action, rest) = take(rest, action_len)?;
        let (signature, rest) = take(rest, 64)?;
        Some((
            SignedAction {
                author: PlayerId(author.try_into().unwrap()),
                seq: u64::from_le_bytes(seq.try_into().unwrap()),
                action: Action::decode(action)?,
                signature: signature.try_into().unwrap(),
            },
            rest,
        ))
    }

    /// Decode exactly one signed action, rejecting trailing bytes.
    pub fn decode_exact(bytes: &[u8]) -> Option<SignedAction> {
        match Self::decode(bytes)? {
            (signed, []) => Some(signed),
            _ => None,
        }
    }

    /// Check the signature against the author's key.
    ///
    /// This is the Reality Kernel gate: an action may reach the world only
    /// if this passes (plus the sequence check in [`crate::ActionLog`]).
    pub fn verify(&self) -> Result<(), VerifyError> {
        let key =
            VerifyingKey::from_bytes(&self.author.0).map_err(|_| VerifyError::BadAuthorKey)?;
        let payload = signing_payload(&self.author, self.seq, &self.action);
        key.verify_strict(&payload, &Signature::from_bytes(&self.signature))
            .map_err(|_| VerifyError::BadSignature)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VerifyError {
    /// The author bytes are not a valid Ed25519 public key.
    BadAuthorKey,
    /// The signature does not match the signed payload.
    BadSignature,
    /// The sequence number is not strictly greater than the author's last
    /// accepted one (a replayed or reordered action).
    SeqReplay { author: PlayerId, seq: u64 },
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyError::BadAuthorKey => write!(f, "author is not a valid Ed25519 key"),
            VerifyError::BadSignature => write!(f, "signature does not match payload"),
            VerifyError::SeqReplay { author, seq } => {
                write!(f, "replayed seq {seq} for author {}", author.short())
            }
        }
    }
}

impl std::error::Error for VerifyError {}
