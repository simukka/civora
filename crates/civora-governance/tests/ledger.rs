//! Accepted-proposal ledger tests: verify-on-append, dedup, rule-version
//! numbering, and strict persistence round-trips.

use std::collections::BTreeMap;

use civora_governance::{
    Cid, GovernanceChange, Ledger, LedgerEntry, LedgerError, LedgerFileError, Proposal,
    ProposalKind, RollbackPlan, SignedCertificate, SignedProposal, SignedVote, Vote, VoteChoice,
};
use civora_identity::{Identity, PlayerId};

fn identity(seed: u8) -> Identity {
    Identity::from_seed([seed; 32])
}

fn cid(byte: u8) -> Cid {
    Cid([byte; 32])
}

fn asset_proposal(author: &Identity, epoch: u64) -> Proposal {
    Proposal {
        kind: ProposalKind::AssetPatch,
        author_public_key: author.player_id(),
        git_commit_hash: [0x11; 20],
        source_bundle_cid: cid(20),
        build_manifest_cid: cid(21),
        wasm_module_cids: vec![],
        asset_cids: vec![cid(epoch as u8 | 0x80)],
        migration_cids: vec![],
        governance_change: None,
        test_results_cid: cid(23),
        activation_epoch: epoch,
        rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
    }
}

fn governance_proposal(author: &Identity, epoch: u64) -> Proposal {
    Proposal {
        kind: ProposalKind::Governance,
        author_public_key: author.player_id(),
        git_commit_hash: [0x22; 20],
        source_bundle_cid: cid(30),
        build_manifest_cid: cid(31),
        wasm_module_cids: vec![],
        asset_cids: vec![],
        migration_cids: vec![],
        governance_change: Some(GovernanceChange {
            rule_module_cid: cid(epoch as u8 | 0x40),
        }),
        test_results_cid: cid(33),
        activation_epoch: epoch,
        rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
    }
}

/// A ledger entry: `author` proposes, is the sole roster member, and votes yes,
/// so any kind clears quorum. `rule_version` is the version the certificate
/// claims (must match the ledger's at append time).
fn entry(author: &Identity, proposal: Proposal, rule_version: u32) -> LedgerEntry {
    let roster = vec![author.player_id()];
    let ballots: BTreeMap<PlayerId, SignedVote> = [(
        author.player_id(),
        SignedVote::sign(
            author,
            Vote {
                proposal_id: proposal.id(),
                voter: author.player_id(),
                choice: VoteChoice::Yes,
            },
        ),
    )]
    .into();
    let epoch = proposal.activation_epoch;
    let certificate =
        SignedCertificate::certify(author, &proposal, &roster, &ballots, rule_version, epoch)
            .expect("sole yes voter clears quorum");
    LedgerEntry {
        proposal: SignedProposal::sign(author, proposal),
        certificate,
    }
}

#[test]
fn append_accepts_and_dedups() {
    let author = identity(1);
    let entry = entry(&author, asset_proposal(&author, 5), 1);
    let id = entry.proposal.proposal_id();

    let mut ledger = Ledger::default();
    assert!(ledger.is_empty());
    assert_eq!(ledger.append(entry.clone()), Ok(true));
    assert!(ledger.contains(&id));
    assert_eq!(ledger.get(&id), Some(&entry));
    assert_eq!(ledger.len(), 1);

    // A second copy of the same proposal is a no-op: first certificate wins.
    assert_eq!(ledger.append(entry.clone()), Ok(false));
    assert_eq!(ledger.len(), 1);
}

#[test]
fn append_rejects_tampered_proposal_and_certificate() {
    let author = identity(1);

    let mut bad_proposal = entry(&author, asset_proposal(&author, 5), 1);
    bad_proposal.proposal.signature[0] ^= 1;
    assert!(matches!(
        Ledger::default().append(bad_proposal),
        Err(LedgerError::BadProposal(_))
    ));

    let mut bad_cert = entry(&author, asset_proposal(&author, 5), 1);
    bad_cert.certificate.signature[0] ^= 1;
    assert!(matches!(
        Ledger::default().append(bad_cert),
        Err(LedgerError::BadCertificate(_))
    ));
}

#[test]
fn rule_version_increments_on_governance_entries() {
    let author = identity(1);
    let mut ledger = Ledger::default();
    assert_eq!(ledger.rule_version(), 1);

    // A non-governance entry leaves the rule version untouched.
    assert_eq!(
        ledger.append(entry(&author, asset_proposal(&author, 5), 1)),
        Ok(true)
    );
    assert_eq!(ledger.rule_version(), 1);

    // A governance entry certified at version 1 lands and bumps to 2.
    assert_eq!(
        ledger.append(entry(&author, governance_proposal(&author, 6), 1)),
        Ok(true)
    );
    assert_eq!(ledger.rule_version(), 2);

    // A second governance proposal whose certificate still claims version 1 is
    // now stale and rejected.
    let stale = entry(&author, governance_proposal(&author, 7), 1);
    assert!(matches!(
        ledger.append(stale),
        Err(LedgerError::BadCertificate(_))
    ));
    assert_eq!(ledger.rule_version(), 2);

    // Certified at the current version 2, it lands and bumps to 3.
    assert_eq!(
        ledger.append(entry(&author, governance_proposal(&author, 7), 2)),
        Ok(true)
    );
    assert_eq!(ledger.rule_version(), 3);
}

fn populated_ledger() -> Ledger {
    let author = identity(1);
    let mut ledger = Ledger::default();
    ledger
        .append(entry(&author, asset_proposal(&author, 5), 1))
        .unwrap();
    ledger
        .append(entry(&author, governance_proposal(&author, 6), 1))
        .unwrap();
    ledger
}

#[test]
fn save_and_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("genesis-0.ledger");
    let ledger = populated_ledger();
    ledger.save(&path).unwrap();

    let loaded = Ledger::load(&path).unwrap();
    assert_eq!(loaded, ledger);
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded.rule_version(), 2);
}

#[test]
fn load_missing_file_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::load(&dir.path().join("does-not-exist.ledger")).unwrap();
    assert!(ledger.is_empty());
}

#[test]
fn load_rejects_corrupt_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("genesis-0.ledger");
    populated_ledger().save(&path).unwrap();
    let good = std::fs::read(&path).unwrap();

    // Bad magic.
    let mut bad_magic = good.clone();
    bad_magic[0] = b'X';
    std::fs::write(&path, &bad_magic).unwrap();
    assert!(matches!(
        Ledger::load(&path),
        Err(LedgerFileError::BadMagic)
    ));

    // Truncated tail.
    std::fs::write(&path, &good[..good.len() - 1]).unwrap();
    assert!(matches!(
        Ledger::load(&path),
        Err(LedgerFileError::Malformed)
    ));

    // Trailing garbage.
    let mut trailing = good.clone();
    trailing.push(0xff);
    std::fs::write(&path, &trailing).unwrap();
    assert!(matches!(
        Ledger::load(&path),
        Err(LedgerFileError::Malformed)
    ));

    // A flipped byte inside an entry breaks a signature.
    let mut flipped = good.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0xff;
    std::fs::write(&path, &flipped).unwrap();
    assert!(Ledger::load(&path).is_err());
}
