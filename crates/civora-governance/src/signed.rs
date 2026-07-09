use civora_identity::{Identity, VerifyError, verify_payload};

use crate::proposal::{MAX_PROPOSAL_BYTES, Proposal, ProposalId};

/// Domain-separation prefix for proposal signatures, so a signature over a
/// proposal can never be confused with one over an action (or any future
/// message type).
pub const PROPOSAL_SIGN_DOMAIN: &[u8] = b"civora.proposal.v1";

/// A [`Proposal`] bound to its author by an Ed25519 signature over the
/// canonical payload `domain || Proposal::encode`.
///
/// Unlike [`civora_identity::SignedAction`] there is no sequence number:
/// replaying a proposal reproduces the same bytes and thus the same
/// [`ProposalId`], and deduplication is the accepted-proposal ledger's job.
/// The author is committed as a field inside the encoding
/// (`author_public_key`) rather than alongside it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SignedProposal {
    pub proposal: Proposal,
    pub signature: [u8; 64],
}

fn signing_payload(proposal: &Proposal) -> Vec<u8> {
    let mut payload = Vec::with_capacity(PROPOSAL_SIGN_DOMAIN.len() + 256);
    payload.extend_from_slice(PROPOSAL_SIGN_DOMAIN);
    proposal.encode(&mut payload);
    payload
}

impl SignedProposal {
    /// Sign `proposal` as `identity`.
    ///
    /// Panics if `proposal.author_public_key` is not `identity.player_id()`:
    /// the author is a committed manifest field, and signing someone else's
    /// authorship claim is always a caller bug.
    pub fn sign(identity: &Identity, proposal: Proposal) -> SignedProposal {
        assert_eq!(
            proposal.author_public_key,
            identity.player_id(),
            "proposal author does not match signing identity"
        );
        let signature = identity.sign_payload(&signing_payload(&proposal));
        SignedProposal {
            proposal,
            signature,
        }
    }

    /// Append the canonical wire encoding of this signed proposal to `out`:
    /// `proposal_len (u32 LE) || proposal bytes || signature (64)`.
    ///
    /// The length prefix makes the encoding self-delimiting so lists of
    /// signed proposals decode by iteration.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut proposal = Vec::with_capacity(256);
        self.proposal.encode(&mut proposal);
        out.extend_from_slice(&(proposal.len() as u32).to_le_bytes());
        out.extend_from_slice(&proposal);
        out.extend_from_slice(&self.signature);
    }

    /// Decode one signed proposal from the front of `bytes`, returning it
    /// and the remaining bytes.
    ///
    /// Returns `None` for truncated input, a length prefix over
    /// [`MAX_PROPOSAL_BYTES`], an inner proposal that [`Proposal::decode`]
    /// rejects, or inner bytes the proposal does not fill exactly. Decoding
    /// checks structure only — call [`SignedProposal::verify`] and
    /// [`Proposal::validate`] before trusting the result.
    pub fn decode(bytes: &[u8]) -> Option<(SignedProposal, &[u8])> {
        fn take(bytes: &[u8], n: usize) -> Option<(&[u8], &[u8])> {
            (bytes.len() >= n).then(|| bytes.split_at(n))
        }

        let (len, rest) = take(bytes, 4)?;
        let proposal_len = u32::from_le_bytes(len.try_into().unwrap()) as usize;
        if proposal_len > MAX_PROPOSAL_BYTES {
            return None;
        }
        let (proposal, rest) = take(rest, proposal_len)?;
        let (signature, rest) = take(rest, 64)?;
        Some((
            SignedProposal {
                proposal: Proposal::decode_exact(proposal)?,
                signature: signature.try_into().unwrap(),
            },
            rest,
        ))
    }

    /// Decode exactly one signed proposal, rejecting trailing bytes.
    pub fn decode_exact(bytes: &[u8]) -> Option<SignedProposal> {
        match Self::decode(bytes)? {
            (signed, []) => Some(signed),
            _ => None,
        }
    }

    /// Check the signature against the manifest's `author_public_key`.
    ///
    /// This is the governance gate: a proposal may reach a vote only if
    /// this passes (plus [`Proposal::validate`]).
    pub fn verify(&self) -> Result<(), VerifyError> {
        verify_payload(
            &self.proposal.author_public_key,
            &signing_payload(&self.proposal),
            &self.signature,
        )
    }

    /// The content-derived id of the inner proposal.
    pub fn proposal_id(&self) -> ProposalId {
        self.proposal.id()
    }
}
