//! Vote format and signature tests: exact layout, canonical decoding, and
//! domain separation from actions and proposals.

use civora_governance::{
    MAX_SIGNED_VOTE_BYTES, ProposalId, SignedVote, VOTE_BYTES, VOTE_SIGN_DOMAIN, Vote, VoteChoice,
};
use civora_identity::{Identity, PlayerId, VerifyError};

fn identity() -> Identity {
    Identity::from_seed([7; 32])
}

fn vote(choice: VoteChoice) -> Vote {
    Vote {
        proposal_id: ProposalId([0xAB; 32]),
        voter: identity().player_id(),
        choice,
    }
}

fn signed(choice: VoteChoice) -> SignedVote {
    SignedVote::sign(&identity(), vote(choice))
}

#[test]
fn vote_round_trips_with_exact_layout() {
    for choice in [VoteChoice::No, VoteChoice::Yes] {
        let vote = vote(choice);
        let mut bytes = Vec::new();
        vote.encode(&mut bytes);
        assert_eq!(bytes.len(), VOTE_BYTES);
        assert_eq!(bytes[0], 1, "format version");
        assert_eq!(&bytes[1..33], &vote.proposal_id.0);
        assert_eq!(&bytes[33..65], &vote.voter.0);
        assert_eq!(
            bytes[65],
            match choice {
                VoteChoice::No => 0,
                VoteChoice::Yes => 1,
            }
        );
        assert_eq!(Vote::decode_exact(&bytes), Some(vote));
    }
}

#[test]
fn signed_vote_round_trips_with_exact_layout() {
    let signed = signed(VoteChoice::Yes);
    let mut bytes = Vec::new();
    signed.encode(&mut bytes);
    assert_eq!(bytes.len(), MAX_SIGNED_VOTE_BYTES);
    assert_eq!(&bytes[VOTE_BYTES..], &signed.signature);
    assert_eq!(SignedVote::decode_exact(&bytes), Some(signed));
    assert!(signed.verify().is_ok());
}

#[test]
fn decode_rejects_truncation_and_trailing_bytes() {
    let signed = signed(VoteChoice::No);
    let mut bytes = Vec::new();
    signed.encode(&mut bytes);

    for len in 0..bytes.len() {
        assert_eq!(SignedVote::decode_exact(&bytes[..len]), None, "len {len}");
    }
    bytes.push(0);
    assert_eq!(SignedVote::decode_exact(&bytes), None);

    // decode (non-exact) hands back the remainder instead.
    let (decoded, rest) = SignedVote::decode(&bytes).unwrap();
    assert_eq!(decoded, signed);
    assert_eq!(rest, &[0]);
}

#[test]
fn decode_rejects_unknown_version_and_choice() {
    let mut bytes = Vec::new();
    signed(VoteChoice::Yes).encode(&mut bytes);

    let mut bad_version = bytes.clone();
    bad_version[0] = 2;
    assert_eq!(SignedVote::decode_exact(&bad_version), None);

    let mut bad_choice = bytes.clone();
    bad_choice[65] = 2;
    assert_eq!(SignedVote::decode_exact(&bad_choice), None);
    bad_choice[65] = 0xff;
    assert_eq!(SignedVote::decode_exact(&bad_choice), None);
}

#[test]
fn tampering_breaks_the_signature() {
    let signed = signed(VoteChoice::Yes);

    // Flip the choice: same voter, different ballot.
    let mut flipped = signed;
    flipped.vote.choice = VoteChoice::No;
    assert_eq!(flipped.verify(), Err(VerifyError::BadSignature));

    // Corrupt one signature byte.
    let mut bad_sig = signed;
    bad_sig.signature[0] ^= 1;
    assert_eq!(bad_sig.verify(), Err(VerifyError::BadSignature));

    // Re-point at a different proposal.
    let mut moved = signed;
    moved.vote.proposal_id = ProposalId([0xCD; 32]);
    assert_eq!(moved.verify(), Err(VerifyError::BadSignature));

    // Claim a different voter (a valid key that didn't sign).
    let mut stolen = signed;
    stolen.vote.voter = Identity::from_seed([8; 32]).player_id();
    assert_eq!(stolen.verify(), Err(VerifyError::BadSignature));

    // A voter id that is not a valid Ed25519 point at all.
    let mut garbage = signed;
    garbage.vote.voter = PlayerId([2; 32]);
    assert_eq!(garbage.verify(), Err(VerifyError::BadAuthorKey));
}

#[test]
fn vote_signatures_are_domain_separated() {
    let identity = identity();
    let vote = vote(VoteChoice::Yes);
    let mut encoded = Vec::new();
    vote.encode(&mut encoded);

    // Signatures over the same vote bytes under other domains must not
    // verify as votes.
    for domain in [
        civora_identity::ACTION_SIGN_DOMAIN,
        civora_governance::PROPOSAL_SIGN_DOMAIN,
        b"".as_slice(),
    ] {
        let mut payload = domain.to_vec();
        payload.extend_from_slice(&encoded);
        let cross = SignedVote {
            vote,
            signature: identity.sign_payload(&payload),
        };
        assert_eq!(cross.verify(), Err(VerifyError::BadSignature));
    }

    // And the genuine payload does verify, pinning the domain constant.
    let mut payload = VOTE_SIGN_DOMAIN.to_vec();
    payload.extend_from_slice(&encoded);
    let genuine = SignedVote {
        vote,
        signature: identity.sign_payload(&payload),
    };
    assert!(genuine.verify().is_ok());
    assert_eq!(VOTE_SIGN_DOMAIN, b"civora.vote.v1");
}
