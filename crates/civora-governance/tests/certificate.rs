//! Finality certificate tests: exact layout, canonical decoding, the full
//! verify rejection matrix, cross-domain separation, the quorum table, and a
//! pinned golden vector.

use std::collections::BTreeMap;

use civora_governance::Cid;
use civora_governance::{
    CERT_SIGN_DOMAIN, CertificateError, FinalityCertificate, MAX_ROSTER_PLAYERS, Proposal,
    ProposalKind, QuorumResult, RollbackPlan, SignedCertificate, SignedVote, Vote, VoteChoice,
    quorum_passes,
};
use civora_identity::{Identity, PlayerId};
use sha2::{Digest, Sha256};

/// Deterministic test identities (not secrets). Distinct seeds so their public
/// keys — and thus roster ordering — differ.
fn identity(seed: u8) -> Identity {
    Identity::from_seed([seed; 32])
}

fn cid(byte: u8) -> Cid {
    Cid([byte; 32])
}

/// A minimal, majority-kind proposal (asset patch) authored by seed 1.
fn proposal() -> Proposal {
    Proposal {
        kind: ProposalKind::AssetPatch,
        author_public_key: identity(1).player_id(),
        git_commit_hash: [0x11; 20],
        source_bundle_cid: cid(20),
        build_manifest_cid: cid(21),
        wasm_module_cids: vec![],
        asset_cids: vec![cid(22)],
        migration_cids: vec![],
        governance_change: None,
        test_results_cid: cid(23),
        activation_epoch: 5,
        rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
    }
}

fn ballot(id: &Identity, proposal: &Proposal, choice: VoteChoice) -> SignedVote {
    SignedVote::sign(
        id,
        Vote {
            proposal_id: proposal.id(),
            voter: id.player_id(),
            choice,
        },
    )
}

fn ballots(
    entries: &[(&Identity, VoteChoice)],
    proposal: &Proposal,
) -> BTreeMap<PlayerId, SignedVote> {
    entries
        .iter()
        .map(|(id, choice)| (id.player_id(), ballot(id, proposal, *choice)))
        .collect()
}

/// A valid two-of-two yes certificate for the majority-kind proposal, certified
/// by seed 1.
fn valid_cert() -> (Proposal, SignedCertificate) {
    let proposal = proposal();
    let a = identity(1);
    let b = identity(2);
    let roster = vec![a.player_id(), b.player_id()];
    let ballots = ballots(&[(&a, VoteChoice::Yes), (&b, VoteChoice::Yes)], &proposal);
    let cert = SignedCertificate::certify(&a, &proposal, &roster, &ballots, 1, 5)
        .expect("two yes over roster of two passes majority");
    (proposal, cert)
}

/// Sign a certificate without the roster-membership assertion in
/// [`SignedCertificate::sign`], so tests can build attributable-but-invalid
/// certificates (e.g. a certifier outside its own roster).
fn sign_raw(identity: &Identity, certificate: FinalityCertificate) -> SignedCertificate {
    let certifier = identity.player_id();
    let mut payload = CERT_SIGN_DOMAIN.to_vec();
    payload.extend_from_slice(&certifier.0);
    let mut enc = Vec::new();
    certificate.encode(&mut enc);
    payload.extend_from_slice(&enc);
    let signature = identity.sign_payload(&payload);
    SignedCertificate {
        certifier,
        certificate,
        signature,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn certificate_round_trips_and_verifies() {
    let (proposal, signed) = valid_cert();

    let mut bytes = Vec::new();
    signed.encode(&mut bytes);
    assert_eq!(
        SignedCertificate::decode_exact(&bytes),
        Some(signed.clone())
    );

    // Inner certificate round-trips on its own too.
    let mut inner = Vec::new();
    signed.certificate.encode(&mut inner);
    assert_eq!(inner[0], 1, "format version");
    assert_eq!(
        FinalityCertificate::decode_exact(&inner),
        Some(signed.certificate.clone())
    );

    assert_eq!(signed.verify(&proposal, 1), Ok(()));
}

#[test]
fn decode_rejects_truncation_and_trailing_bytes() {
    let (_, signed) = valid_cert();
    let mut bytes = Vec::new();
    signed.encode(&mut bytes);

    for len in 0..bytes.len() {
        assert_eq!(
            SignedCertificate::decode_exact(&bytes[..len]),
            None,
            "truncated at {len}"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(SignedCertificate::decode_exact(&trailing), None);

    // Non-exact decode hands back the remainder.
    let (decoded, rest) = SignedCertificate::decode(&trailing).unwrap();
    assert_eq!(decoded, signed);
    assert_eq!(rest, &[0]);
}

#[test]
fn decode_rejects_unknown_version_and_quorum_byte() {
    let (_, signed) = valid_cert();
    let mut inner = Vec::new();
    signed.certificate.encode(&mut inner);

    let mut bad_version = inner.clone();
    bad_version[0] = 2;
    assert_eq!(FinalityCertificate::decode_exact(&bad_version), None);

    // Quorum byte sits right after version(1) + proposal_id(32) +
    // rule_version(4) + accepted_epoch(8) = offset 45.
    let mut bad_quorum = inner.clone();
    assert_eq!(bad_quorum[45], 1, "quorum byte position");
    bad_quorum[45] = 0;
    assert_eq!(FinalityCertificate::decode_exact(&bad_quorum), None);
    bad_quorum[45] = 2;
    assert_eq!(FinalityCertificate::decode_exact(&bad_quorum), None);
}

#[test]
fn decode_rejects_over_cap_and_non_canonical_lists() {
    // A roster count over the cap is rejected without allocating it.
    let mut over_cap = Vec::new();
    over_cap.push(1u8); // version
    over_cap.extend_from_slice(&[0u8; 32]); // proposal id
    over_cap.extend_from_slice(&1u32.to_le_bytes()); // rule version
    over_cap.extend_from_slice(&5u64.to_le_bytes()); // accepted epoch
    over_cap.push(1u8);
    over_cap.extend_from_slice(&((MAX_ROSTER_PLAYERS as u16) + 1).to_le_bytes());
    assert_eq!(FinalityCertificate::decode_exact(&over_cap), None);

    // A non-ascending roster is non-canonical.
    let a = identity(1);
    let b = identity(2);
    let (lo, hi) = if a.player_id() < b.player_id() {
        (a.player_id(), b.player_id())
    } else {
        (b.player_id(), a.player_id())
    };
    let mut descending = Vec::new();
    descending.push(1u8);
    descending.extend_from_slice(&[0u8; 32]);
    descending.extend_from_slice(&1u32.to_le_bytes());
    descending.extend_from_slice(&5u64.to_le_bytes());
    descending.push(1u8);
    descending.extend_from_slice(&2u16.to_le_bytes());
    descending.extend_from_slice(&hi.0);
    descending.extend_from_slice(&lo.0);
    descending.extend_from_slice(&0u16.to_le_bytes()); // yes
    descending.extend_from_slice(&0u16.to_le_bytes()); // no
    assert_eq!(FinalityCertificate::decode_exact(&descending), None);

    // An empty roster is rejected.
    let mut empty_roster = Vec::new();
    empty_roster.push(1u8);
    empty_roster.extend_from_slice(&[0u8; 32]);
    empty_roster.extend_from_slice(&1u32.to_le_bytes());
    empty_roster.extend_from_slice(&5u64.to_le_bytes());
    empty_roster.push(1u8);
    empty_roster.extend_from_slice(&0u16.to_le_bytes()); // roster
    empty_roster.extend_from_slice(&0u16.to_le_bytes()); // yes
    empty_roster.extend_from_slice(&0u16.to_le_bytes()); // no
    assert_eq!(FinalityCertificate::decode_exact(&empty_roster), None);
}

#[test]
fn verify_rejects_proposal_mismatch() {
    let (proposal, signed) = valid_cert();
    let mut other = proposal.clone();
    other.activation_epoch = 6; // changes the id
    assert_ne!(other.id(), proposal.id());
    assert_eq!(
        signed.verify(&other, 1),
        Err(CertificateError::ProposalMismatch)
    );
}

#[test]
fn verify_rejects_tampered_certifier() {
    let (proposal, signed) = valid_cert();

    // Corrupted signature.
    let mut bad_sig = signed.clone();
    bad_sig.signature[0] ^= 1;
    assert_eq!(
        bad_sig.verify(&proposal, 1),
        Err(CertificateError::BadCertifierSignature)
    );

    // A different (valid) certifier key that did not sign this payload.
    let mut stolen = signed.clone();
    stolen.certifier = identity(2).player_id();
    assert_eq!(
        stolen.verify(&proposal, 1),
        Err(CertificateError::BadCertifierSignature)
    );
}

#[test]
fn verify_rejects_certifier_outside_roster() {
    let proposal = proposal();
    let outsider = identity(9);
    let member = identity(2);
    // Roster is just the member; the outsider certifies anyway.
    let cert = FinalityCertificate {
        proposal_id: proposal.id(),
        governance_rule_version: 1,
        accepted_epoch: 5,
        quorum_result: QuorumResult::Accepted,
        eligible_roster: vec![member.player_id()],
        yes_votes: vec![(
            member.player_id(),
            ballot(&member, &proposal, VoteChoice::Yes).signature,
        )],
        no_votes: vec![],
    };
    let signed = sign_raw(&outsider, cert);
    assert_eq!(
        signed.verify(&proposal, 1),
        Err(CertificateError::CertifierNotInRoster)
    );
}

#[test]
fn verify_rejects_non_roster_voter() {
    let proposal = proposal();
    let certifier = identity(1);
    let stranger = identity(9);
    let cert = FinalityCertificate {
        proposal_id: proposal.id(),
        governance_rule_version: 1,
        accepted_epoch: 5,
        quorum_result: QuorumResult::Accepted,
        eligible_roster: vec![certifier.player_id()],
        yes_votes: vec![(
            stranger.player_id(),
            ballot(&stranger, &proposal, VoteChoice::Yes).signature,
        )],
        no_votes: vec![],
    };
    let signed = sign_raw(&certifier, cert);
    assert_eq!(
        signed.verify(&proposal, 1),
        Err(CertificateError::VoterNotInRoster)
    );
}

#[test]
fn verify_rejects_voter_in_both_lists() {
    let proposal = proposal();
    let certifier = identity(1);
    let voter = identity(2);
    let cert = FinalityCertificate {
        proposal_id: proposal.id(),
        governance_rule_version: 1,
        accepted_epoch: 5,
        quorum_result: QuorumResult::Accepted,
        eligible_roster: {
            let mut r = vec![certifier.player_id(), voter.player_id()];
            r.sort();
            r
        },
        yes_votes: vec![(
            voter.player_id(),
            ballot(&voter, &proposal, VoteChoice::Yes).signature,
        )],
        no_votes: vec![(
            voter.player_id(),
            ballot(&voter, &proposal, VoteChoice::No).signature,
        )],
    };
    let signed = sign_raw(&certifier, cert);
    assert_eq!(
        signed.verify(&proposal, 1),
        Err(CertificateError::VoterInBothLists)
    );
}

#[test]
fn verify_rejects_tampered_vote_signature() {
    let (proposal, signed) = valid_cert();
    // Corrupt a vote signature, then re-sign so the certifier signature is
    // valid over the tampered bytes and the vote check is what fails.
    let mut cert = signed.certificate.clone();
    cert.yes_votes[0].1 = [0u8; 64];
    let resigned = sign_raw(&identity(1), cert);
    assert_eq!(
        resigned.verify(&proposal, 1),
        Err(CertificateError::BadVoteSignature)
    );
}

#[test]
fn verify_rejects_failed_quorum() {
    let proposal = proposal();
    let certifier = identity(1);
    // Roster of three, a single yes: 1*2 > 3 is false.
    let mut roster = vec![
        certifier.player_id(),
        identity(2).player_id(),
        identity(3).player_id(),
    ];
    roster.sort();
    let cert = FinalityCertificate {
        proposal_id: proposal.id(),
        governance_rule_version: 1,
        accepted_epoch: 5,
        quorum_result: QuorumResult::Accepted,
        eligible_roster: roster,
        yes_votes: vec![(
            certifier.player_id(),
            ballot(&certifier, &proposal, VoteChoice::Yes).signature,
        )],
        no_votes: vec![],
    };
    let signed = sign_raw(&certifier, cert);
    assert_eq!(
        signed.verify(&proposal, 1),
        Err(CertificateError::QuorumNotMet)
    );
}

#[test]
fn verify_rejects_early_epoch() {
    let proposal = proposal(); // activation_epoch = 5
    let a = identity(1);
    let b = identity(2);
    let roster = vec![a.player_id(), b.player_id()];
    let ballots = ballots(&[(&a, VoteChoice::Yes), (&b, VoteChoice::Yes)], &proposal);
    // accepted_epoch 4 < activation 5.
    let signed = SignedCertificate::certify(&a, &proposal, &roster, &ballots, 1, 4).unwrap();
    assert_eq!(
        signed.verify(&proposal, 1),
        Err(CertificateError::EpochTooEarly)
    );
    // Equal is fine (offline peer certifying after close from stored ballots).
    let at_close = SignedCertificate::certify(&a, &proposal, &roster, &ballots, 1, 5).unwrap();
    assert_eq!(at_close.verify(&proposal, 1), Ok(()));
}

#[test]
fn verify_rejects_wrong_rule_version() {
    let (proposal, signed) = valid_cert(); // rule version 1
    assert_eq!(
        signed.verify(&proposal, 2),
        Err(CertificateError::RuleVersionMismatch)
    );
}

#[test]
fn certificate_signatures_are_domain_separated() {
    assert_eq!(CERT_SIGN_DOMAIN, b"civora.certificate.v1");
    let (proposal, signed) = valid_cert();
    let identity = identity(1);

    // A signature over the same certifier+certificate bytes under any other
    // domain must not verify as a certificate.
    let mut cert_bytes = Vec::new();
    signed.certificate.encode(&mut cert_bytes);
    for domain in [
        civora_identity::ACTION_SIGN_DOMAIN,
        civora_governance::PROPOSAL_SIGN_DOMAIN,
        civora_governance::VOTE_SIGN_DOMAIN,
        b"".as_slice(),
    ] {
        let mut payload = domain.to_vec();
        payload.extend_from_slice(&identity.player_id().0);
        payload.extend_from_slice(&cert_bytes);
        let cross = SignedCertificate {
            certifier: identity.player_id(),
            certificate: signed.certificate.clone(),
            signature: identity.sign_payload(&payload),
        };
        assert_eq!(
            cross.verify(&proposal, 1),
            Err(CertificateError::BadCertifierSignature)
        );
    }
}

#[test]
fn quorum_passes_table() {
    use ProposalKind::*;
    // (kind, roster, yes, no, expected)
    let cases = [
        // Majority kinds: yes * 2 > roster, at least one ballot.
        (AssetPatch, 1, 1, 0, true),
        (AssetPatch, 1, 0, 0, false), // zero ballots
        (AssetPatch, 2, 1, 0, false), // 2 > 2 is false (tie)
        (AssetPatch, 2, 2, 0, true),
        (AssetPatch, 3, 2, 0, true),  // 4 > 3
        (AssetPatch, 3, 1, 1, false), // 2 > 3 false
        (AssetPatch, 4, 2, 1, false), // 4 > 4 tie
        (AssetPatch, 4, 3, 0, true),
        (AssetPatch, 5, 3, 0, true),  // 6 > 5
        (AssetPatch, 5, 2, 0, false), // 4 > 5 false
        // Supermajority kinds: yes * 3 > roster * 2.
        (Economy, 3, 2, 0, false), // 6 > 6 boundary — not strict
        (Economy, 3, 3, 0, true),
        (Governance, 3, 2, 1, false), // 6 > 6 boundary
        (Governance, 3, 3, 0, true),
        (Economy, 4, 3, 0, true),     // 9 > 8
        (Economy, 4, 2, 0, false),    // 6 > 8 false
        (Governance, 5, 4, 0, true),  // 12 > 10
        (Governance, 5, 3, 0, false), // 9 > 10 false
        (Governance, 1, 1, 0, true),  // 3 > 2
        // Kernel never passes, even unanimous.
        (Kernel, 1, 1, 0, false),
        (Kernel, 3, 3, 0, false),
    ];
    for (kind, roster, yes, no, expected) in cases {
        assert_eq!(
            quorum_passes(kind, roster, yes, no),
            expected,
            "{kind:?} roster {roster} yes {yes} no {no}"
        );
    }
}

#[test]
fn golden_vector() {
    // A pinned fixture: two-of-two yes certificate, certifier seed 1. Ed25519
    // signatures are deterministic, so these bytes are stable. If this test
    // fails, the wire format or a signing input changed — do not "fix" it by
    // updating the constants without understanding why.
    let (_, signed) = valid_cert();

    let root = signed.certificate.eligible_roster_root();
    assert_eq!(
        hex(&root),
        "a390193fcc178618d65339f2277a400e7df58f8828d7885620e3689dc4b09a96",
        "eligible_roster_root"
    );

    let mut bytes = Vec::new();
    signed.encode(&mut bytes);
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    assert_eq!(
        hex(&digest),
        "680af941d45ff2ef9ad27874fc2bfd77fa7aec8d305a01d6906b4aa7bc5d39b7",
        "signed certificate encoding"
    );
}
