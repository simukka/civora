use civora_governance::{
    Cid, GovernanceChange, MAX_PROPOSAL_BYTES, Proposal, ProposalKind, RollbackPlan,
    SignedProposal, ValidationError,
};
use civora_identity::{ACTION_SIGN_DOMAIN, Identity, VerifyError};

/// Deterministic test identity (not a secret).
fn identity() -> Identity {
    Identity::from_seed([7; 32])
}

fn cid(byte: u8) -> Cid {
    Cid([byte; 32])
}

/// Every field populated: governance change, all cid lists, reverse
/// migrations. Also the golden-vector fixture — do not change it.
fn full_proposal() -> Proposal {
    Proposal {
        kind: ProposalKind::Governance,
        author_public_key: identity().player_id(),
        git_commit_hash: [0xab; 20],
        source_bundle_cid: cid(1),
        build_manifest_cid: cid(2),
        wasm_module_cids: vec![cid(3), cid(4)],
        asset_cids: vec![cid(5), cid(6)],
        // Execution order is semantic, not sorted.
        migration_cids: vec![cid(8), cid(7)],
        governance_change: Some(GovernanceChange {
            rule_module_cid: cid(9),
        }),
        test_results_cid: cid(10),
        activation_epoch: 184_220,
        rollback_plan: RollbackPlan::ReverseMigrations {
            reverse_migration_cids: vec![cid(12), cid(11)],
        },
    }
}

/// The smallest valid proposal: an asset-only patch.
fn minimal_proposal() -> Proposal {
    Proposal {
        kind: ProposalKind::AssetPatch,
        author_public_key: identity().player_id(),
        git_commit_hash: [0x11; 20],
        source_bundle_cid: cid(20),
        build_manifest_cid: cid(21),
        wasm_module_cids: vec![],
        asset_cids: vec![cid(22)],
        migration_cids: vec![],
        governance_change: None,
        test_results_cid: cid(23),
        activation_epoch: 1,
        rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
    }
}

fn encoded(proposal: &Proposal) -> Vec<u8> {
    let mut out = Vec::new();
    proposal.encode(&mut out);
    out
}

// Offsets into the canonical layout (see Proposal::encode docs). The fixed
// header is version (1) + kind (1) + author (32) + git commit (20) +
// source bundle cid (32) + build manifest cid (32).
const WASM_LIST_OFFSET: usize = 118;
// The encoding ends with governance tag-dependent bytes; for a proposal
// with governance_change None and snapshot rollback the tail is
// gov tag (1) + test_results_cid (32) + epoch (8) + rollback tag (1).
const GOV_TAG_FROM_END: usize = 42;

#[test]
fn proposal_round_trips() {
    for proposal in [full_proposal(), minimal_proposal()] {
        let bytes = encoded(&proposal);
        let decoded = Proposal::decode_exact(&bytes).expect("canonical bytes decode");
        assert_eq!(decoded, proposal);
        assert_eq!(decoded.id(), proposal.id());
    }
}

#[test]
fn proposal_decode_rejects_malformed() {
    let bytes = encoded(&full_proposal());
    for len in 0..bytes.len() {
        assert!(
            Proposal::decode(&bytes[..len]).is_none(),
            "truncation at {len} must not decode"
        );
    }

    let mut trailing = bytes.clone();
    trailing.push(0xff);
    assert!(Proposal::decode_exact(&trailing).is_none());
    let (_, rest) = Proposal::decode(&trailing).expect("list-style decode leaves the rest");
    assert_eq!(rest, [0xff]);

    let mut bad_version = bytes.clone();
    bad_version[0] = 2;
    assert!(Proposal::decode(&bad_version).is_none());

    let mut bad_kind = bytes.clone();
    bad_kind[1] = 6;
    assert!(Proposal::decode(&bad_kind).is_none());

    let minimal = encoded(&minimal_proposal());
    let mut bad_gov_tag = minimal.clone();
    let gov_at = bad_gov_tag.len() - GOV_TAG_FROM_END;
    bad_gov_tag[gov_at] = 2;
    assert!(Proposal::decode(&bad_gov_tag).is_none());

    let mut bad_rollback_tag = minimal.clone();
    *bad_rollback_tag.last_mut().unwrap() = 2;
    assert!(Proposal::decode(&bad_rollback_tag).is_none());
}

#[test]
fn proposal_decode_rejects_noncanonical() {
    // The full fixture's wasm list holds two cids at these byte ranges.
    let bytes = encoded(&full_proposal());
    let first = WASM_LIST_OFFSET + 2;
    let second = first + 32;

    let mut unsorted = bytes.clone();
    for i in 0..32 {
        unsorted.swap(first + i, second + i);
    }
    assert!(Proposal::decode(&unsorted).is_none(), "unsorted set list");

    // Asset list follows the wasm list; duplicate its two entries.
    let asset_first = second + 32 + 2;
    let mut dup_assets = bytes.clone();
    dup_assets.copy_within(asset_first..asset_first + 32, asset_first + 32);
    assert!(Proposal::decode(&dup_assets).is_none(), "duplicate set cid");

    // Migration list follows the asset list.
    let migration_first = asset_first + 64 + 2;
    let mut dup_migrations = bytes.clone();
    dup_migrations.copy_within(migration_first..migration_first + 32, migration_first + 32);
    assert!(
        Proposal::decode(&dup_migrations).is_none(),
        "duplicate migration cid"
    );

    // The reverse-migration list ends the encoding; duplicate its entries.
    let reverse_first = bytes.len() - 64;
    let mut dup_reverse = bytes.clone();
    dup_reverse.copy_within(reverse_first..reverse_first + 32, reverse_first + 32);
    assert!(
        Proposal::decode(&dup_reverse).is_none(),
        "duplicate reverse-migration cid"
    );

    // A list count over the cap must be rejected before allocating.
    let mut bomb = encoded(&minimal_proposal());
    bomb[WASM_LIST_OFFSET..WASM_LIST_OFFSET + 2].copy_from_slice(&1025u16.to_le_bytes());
    assert!(Proposal::decode(&bomb).is_none(), "count over cap");
}

#[test]
fn proposal_id_is_stable_golden_vector() {
    // Pinned when the format was implemented; a change here means the
    // canonical encoding silently drifted and old proposal ids broke.
    assert_eq!(
        full_proposal().id().to_string(),
        "f0c3a801ffb2361c14212e1c56b75f28d960c6cad80f5d2521593a785002d56e"
    );

    let mut changed = full_proposal();
    changed.activation_epoch += 1;
    assert_ne!(changed.id(), full_proposal().id());

    let mut changed = full_proposal();
    changed.kind = ProposalKind::Economy;
    assert_ne!(changed.id(), full_proposal().id());

    // The id commits to the manifest, not the signature.
    let signed = SignedProposal::sign(&identity(), full_proposal());
    assert_eq!(signed.proposal_id(), full_proposal().id());
}

#[test]
fn sign_verify_round_trip() {
    let signed = SignedProposal::sign(&identity(), full_proposal());
    signed.verify().expect("freshly signed proposal verifies");

    let mut bytes = Vec::new();
    signed.encode(&mut bytes);
    let decoded = SignedProposal::decode_exact(&bytes).expect("canonical bytes decode");
    assert_eq!(decoded, signed);
    decoded.verify().expect("decoded proposal still verifies");
}

#[test]
fn tampered_proposal_fails_verification() {
    let mut signed = SignedProposal::sign(&identity(), full_proposal());
    signed.proposal.activation_epoch += 1;
    assert_eq!(signed.verify(), Err(VerifyError::BadSignature));

    let mut signed = SignedProposal::sign(&identity(), full_proposal());
    signed.signature[10] ^= 0x01;
    assert_eq!(signed.verify(), Err(VerifyError::BadSignature));

    // A valid key that didn't sign the payload must not verify.
    let mut signed = SignedProposal::sign(&identity(), full_proposal());
    signed.proposal.author_public_key = Identity::from_seed([9; 32]).player_id();
    assert_eq!(signed.verify(), Err(VerifyError::BadSignature));

    // Author bytes that are not a valid Ed25519 key at all.
    let mut signed = SignedProposal::sign(&identity(), full_proposal());
    signed.proposal.author_public_key.0 = [2; 32];
    assert_eq!(signed.verify(), Err(VerifyError::BadAuthorKey));
}

#[test]
fn action_signature_does_not_verify_as_proposal() {
    // Sign the proposal bytes under the ACTION domain: domain separation
    // must make this signature useless as a proposal signature.
    let proposal = full_proposal();
    let mut payload = ACTION_SIGN_DOMAIN.to_vec();
    proposal.encode(&mut payload);
    let signature = identity().sign_payload(&payload);
    let forged = SignedProposal {
        proposal,
        signature,
    };
    assert_eq!(forged.verify(), Err(VerifyError::BadSignature));
}

#[test]
fn validate_enforces_kind_rules() {
    full_proposal().validate().expect("full fixture is valid");
    minimal_proposal()
        .validate()
        .expect("minimal fixture is valid");

    let mut p = minimal_proposal();
    p.wasm_module_cids = vec![cid(1)];
    assert_eq!(p.validate(), Err(ValidationError::AssetPatchHasCode));

    let mut p = minimal_proposal();
    p.migration_cids = vec![cid(1)];
    assert_eq!(p.validate(), Err(ValidationError::AssetPatchHasCode));

    let mut p = minimal_proposal();
    p.kind = ProposalKind::GameplayCode;
    assert_eq!(p.validate(), Err(ValidationError::GameplayPatchWithoutCode));

    let mut p = minimal_proposal();
    p.kind = ProposalKind::Governance;
    assert_eq!(p.validate(), Err(ValidationError::GovernanceChangeMismatch));

    let mut p = full_proposal();
    p.kind = ProposalKind::Economy;
    assert_eq!(p.validate(), Err(ValidationError::GovernanceChangeMismatch));

    let mut p = minimal_proposal();
    p.kind = ProposalKind::Kernel;
    assert_eq!(
        p.validate(),
        Err(ValidationError::KernelChangeNotHotPatchable)
    );

    let mut p = minimal_proposal();
    p.asset_cids = vec![];
    assert_eq!(p.validate(), Err(ValidationError::EmptyProposal));

    let mut p = minimal_proposal();
    p.asset_cids = vec![cid(2), cid(1)];
    assert_eq!(p.validate(), Err(ValidationError::NonCanonical));

    let mut p = minimal_proposal();
    p.kind = ProposalKind::NewContent;
    p.migration_cids = vec![cid(1), cid(1)];
    assert_eq!(p.validate(), Err(ValidationError::NonCanonical));

    let mut p = full_proposal();
    p.rollback_plan = RollbackPlan::ReverseMigrations {
        reverse_migration_cids: vec![cid(1), cid(1)],
    };
    assert_eq!(p.validate(), Err(ValidationError::NonCanonical));
}

#[test]
fn signed_proposal_decode_rejects_malformed() {
    let signed = SignedProposal::sign(&identity(), full_proposal());
    let mut bytes = Vec::new();
    signed.encode(&mut bytes);

    for len in 0..bytes.len() {
        assert!(
            SignedProposal::decode(&bytes[..len]).is_none(),
            "truncation at {len} must not decode"
        );
    }

    let mut trailing = bytes.clone();
    trailing.push(0xff);
    assert!(SignedProposal::decode_exact(&trailing).is_none());
    let (_, rest) = SignedProposal::decode(&trailing).expect("list-style decode leaves the rest");
    assert_eq!(rest, [0xff]);

    // Length prefix claiming more than the actual proposal: the inner
    // decode sees a trailing signature byte and rejects.
    let inner_len = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let mut over_len = bytes.clone();
    over_len[..4].copy_from_slice(&(inner_len + 1).to_le_bytes());
    over_len.push(0x00);
    assert!(SignedProposal::decode(&over_len).is_none());

    // Length prefix over the size cap must be rejected before allocating.
    let mut bomb = bytes.clone();
    bomb[..4].copy_from_slice(&((MAX_PROPOSAL_BYTES as u32) + 1).to_le_bytes());
    assert!(SignedProposal::decode(&bomb).is_none());
}

#[test]
fn cid_of_is_sha256() {
    // Published SHA-256 vector for empty input.
    assert_eq!(
        Cid::of(b"").to_string(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(Cid::of(b"").short(), "e3b0c442");
}
