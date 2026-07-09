use civora_identity::{Identity, PlayerId, VerifyError, verify_payload};

use crate::proposal::ProposalId;

/// Domain-separation prefix for vote signatures, so a signature over a vote
/// can never be confused with one over an action or a proposal.
pub const VOTE_SIGN_DOMAIN: &[u8] = b"civora.vote.v1";

/// Leading byte of every encoded vote. Like proposals, votes are governance
/// records the accepted-proposal ledger will persist, so the version travels
/// in the bytes themselves.
pub const VOTE_FORMAT_VERSION: u8 = 1;

/// Exact size of an encoded [`Vote`]:
/// `version (1) || proposal_id (32) || voter (32) || choice (1)`.
pub const VOTE_BYTES: usize = 66;

/// Exact size of an encoded [`SignedVote`]: [`VOTE_BYTES`] + signature (64).
pub const MAX_SIGNED_VOTE_BYTES: usize = VOTE_BYTES + 64;

/// A yes/no ballot. Explicit abstention is not a message: not voting is
/// abstaining.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VoteChoice {
    No,
    Yes,
}

impl VoteChoice {
    fn as_byte(self) -> u8 {
        match self {
            VoteChoice::No => 0,
            VoteChoice::Yes => 1,
        }
    }

    fn from_byte(byte: u8) -> Option<VoteChoice> {
        match byte {
            0 => Some(VoteChoice::No),
            1 => Some(VoteChoice::Yes),
            _ => None,
        }
    }
}

/// One player's ballot on one proposal.
///
/// A later vote by the same voter on the same proposal replaces the earlier
/// one (revoting is allowed). In this milestone tallies are display-only;
/// finality (milestone 6) must bind votes to an agreed ordering, since
/// gossip arrival order is not consistent across peers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Vote {
    pub proposal_id: ProposalId,
    /// The voter; must match the signer of the enclosing [`SignedVote`].
    pub voter: PlayerId,
    pub choice: VoteChoice,
}

impl Vote {
    /// Append the canonical encoding of this vote to `out`:
    /// `version (u8 = 1) || proposal_id (32) || voter (32) || choice (u8)`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(VOTE_FORMAT_VERSION);
        out.extend_from_slice(&self.proposal_id.0);
        out.extend_from_slice(&self.voter.0);
        out.push(self.choice.as_byte());
    }

    /// Decode one vote from the front of `bytes`, returning it and the
    /// remaining bytes. Returns `None` for an unknown version or choice, or
    /// truncated input.
    pub fn decode(bytes: &[u8]) -> Option<(Vote, &[u8])> {
        fn take(bytes: &[u8], n: usize) -> Option<(&[u8], &[u8])> {
            (bytes.len() >= n).then(|| bytes.split_at(n))
        }

        let (version, rest) = take(bytes, 1)?;
        if version[0] != VOTE_FORMAT_VERSION {
            return None;
        }
        let (proposal_id, rest) = take(rest, 32)?;
        let (voter, rest) = take(rest, 32)?;
        let (choice, rest) = take(rest, 1)?;
        Some((
            Vote {
                proposal_id: ProposalId(proposal_id.try_into().unwrap()),
                voter: PlayerId(voter.try_into().unwrap()),
                choice: VoteChoice::from_byte(choice[0])?,
            },
            rest,
        ))
    }

    /// Decode exactly one vote, rejecting trailing bytes.
    pub fn decode_exact(bytes: &[u8]) -> Option<Vote> {
        match Self::decode(bytes)? {
            (vote, []) => Some(vote),
            _ => None,
        }
    }
}

/// The domain-separated payload an Ed25519 vote signature covers:
/// `VOTE_SIGN_DOMAIN || Vote::encode`. Exposed to the crate so the finality
/// certificate can verify `(voter, sig)` pairs against a vote it reconstructs
/// from certificate fields, without embedding whole [`SignedVote`]s.
pub(crate) fn signing_payload(vote: &Vote) -> Vec<u8> {
    let mut payload = Vec::with_capacity(VOTE_SIGN_DOMAIN.len() + VOTE_BYTES);
    payload.extend_from_slice(VOTE_SIGN_DOMAIN);
    vote.encode(&mut payload);
    payload
}

/// A [`Vote`] bound to its voter by an Ed25519 signature over the canonical
/// payload `domain || Vote::encode`.
///
/// No sequence number: a replayed vote reproduces the same (proposal, voter,
/// choice) triple and is idempotent under latest-wins tallying.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SignedVote {
    pub vote: Vote,
    pub signature: [u8; 64],
}

impl SignedVote {
    /// Sign `vote` as `identity`.
    ///
    /// Panics if `vote.voter` is not `identity.player_id()`: the voter is a
    /// committed field, and signing someone else's ballot is always a caller
    /// bug.
    pub fn sign(identity: &Identity, vote: Vote) -> SignedVote {
        assert_eq!(
            vote.voter,
            identity.player_id(),
            "vote voter does not match signing identity"
        );
        let signature = identity.sign_payload(&signing_payload(&vote));
        SignedVote { vote, signature }
    }

    /// Append the canonical wire encoding of this signed vote to `out`:
    /// `vote (66) || signature (64)`. Fixed size, so no length prefix.
    pub fn encode(&self, out: &mut Vec<u8>) {
        self.vote.encode(out);
        out.extend_from_slice(&self.signature);
    }

    /// Decode one signed vote from the front of `bytes`, returning it and
    /// the remaining bytes. Decoding checks structure only — call
    /// [`SignedVote::verify`] before trusting the result.
    pub fn decode(bytes: &[u8]) -> Option<(SignedVote, &[u8])> {
        let (vote, rest) = Vote::decode(bytes)?;
        if rest.len() < 64 {
            return None;
        }
        let (signature, rest) = rest.split_at(64);
        Some((
            SignedVote {
                vote,
                signature: signature.try_into().unwrap(),
            },
            rest,
        ))
    }

    /// Decode exactly one signed vote, rejecting trailing bytes.
    pub fn decode_exact(bytes: &[u8]) -> Option<SignedVote> {
        match Self::decode(bytes)? {
            (signed, []) => Some(signed),
            _ => None,
        }
    }

    /// Check the signature against the vote's `voter`.
    ///
    /// This is the tally gate: a vote counts only if this passes.
    pub fn verify(&self) -> Result<(), VerifyError> {
        verify_payload(
            &self.vote.voter,
            &signing_payload(&self.vote),
            &self.signature,
        )
    }
}
