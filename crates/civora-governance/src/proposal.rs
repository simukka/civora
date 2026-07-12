use std::fmt;

use civora_identity::PlayerId;
use sha2::{Digest, Sha256};

use crate::cid::Cid;

/// Domain prefix hashed in front of the canonical encoding to derive a
/// [`ProposalId`], so a proposal id can never collide with a hash of the
/// same bytes in another role.
pub const PROPOSAL_ID_DOMAIN: &[u8] = b"civora.proposal-id.v1";

/// Leading byte of every encoded proposal. Unlike actions and wire messages
/// (whose version rides in gossip topics and the sync handshake), proposals
/// are persisted governance records that the accepted-proposal ledger must
/// decode long after the session that wrote them, so the version travels in
/// the bytes themselves.
pub const PROPOSAL_FORMAT_VERSION: u8 = 1;

/// Cap per content-id list; guards decode against allocation bombs.
pub const MAX_CIDS_PER_LIST: usize = 1024;

/// Documented upper bound on an encoded proposal (all lists full).
pub const MAX_PROPOSAL_BYTES: usize = 192 * 1024;

/// Change classification driving the initial voting-rules table: each kind
/// maps to a different approval requirement (simple majority, higher quorum,
/// activation delay, ...). Carried explicitly so voters can classify a
/// proposal before fetching any of its content.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProposalKind {
    /// Asset-only patch: textures, meshes, sounds, biome configs.
    AssetPatch,
    /// New item, creature, or biome.
    NewContent,
    /// Gameplay code patch (wasm rule modules).
    GameplayCode,
    /// Economy change.
    Economy,
    /// Governance rule change.
    Governance,
    /// Kernel change. Encodes, but [`Proposal::validate`] rejects it:
    /// kernel changes are not in-game hot patches in v1.
    Kernel,
}

impl ProposalKind {
    fn as_byte(self) -> u8 {
        match self {
            ProposalKind::AssetPatch => 0,
            ProposalKind::NewContent => 1,
            ProposalKind::GameplayCode => 2,
            ProposalKind::Economy => 3,
            ProposalKind::Governance => 4,
            ProposalKind::Kernel => 5,
        }
    }

    fn from_byte(byte: u8) -> Option<ProposalKind> {
        Some(match byte {
            0 => ProposalKind::AssetPatch,
            1 => ProposalKind::NewContent,
            2 => ProposalKind::GameplayCode,
            3 => ProposalKind::Economy,
            4 => ProposalKind::Governance,
            5 => ProposalKind::Kernel,
            _ => return None,
        })
    }
}

/// A proposed change to the governance rule itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GovernanceChange {
    /// Content id of the wasm module implementing the proposed voting rule.
    /// Rule version numbering is assigned by the accepted-proposal ledger.
    pub rule_module_cid: Cid,
}

/// What clients do if the patch corrupts a realm. An enum rather than free
/// text so the patch loader can execute it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RollbackPlan {
    /// Revert to the last signed snapshot taken before `activation_epoch`.
    RevertToLastSignedSnapshot,
    /// Run these reverse migrations in order, then unload the patch.
    ReverseMigrations { reverse_migration_cids: Vec<Cid> },
}

/// Content-derived identity of a proposal: SHA-256 over
/// `PROPOSAL_ID_DOMAIN || Proposal::encode()`.
///
/// Never stored inside the proposal — like a git commit hash, any holder
/// recomputes it from the bytes, so an id mismatch is structurally
/// impossible. It commits to every manifest field and deliberately excludes
/// the signature, so it is stable before and after signing. This is the
/// value a finality certificate's `proposal_id` field references.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ProposalId(pub [u8; 32]);

impl ProposalId {
    /// Short display form (first 8 hex chars) for the HUD and logs.
    pub fn short(&self) -> String {
        self.0[..4].iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Display for ProposalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// A proposal manifest: what a git commit becomes on its way to a vote.
///
/// The manifest names the commit and the content-addressed artifacts built
/// from it; the artifacts themselves travel separately (patch packs). All
/// mutation of shared reality flows through proposals — a commit becomes
/// real only when a finality certificate over this manifest's id exists.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Proposal {
    pub kind: ProposalKind,
    /// The proposer; must match the signer of the enclosing
    /// [`crate::SignedProposal`].
    pub author_public_key: PlayerId,
    /// Raw SHA-1 object id of the proposed commit. Provenance metadata —
    /// the content ids below are the integrity anchor.
    pub git_commit_hash: [u8; 20],
    pub source_bundle_cid: Cid,
    pub build_manifest_cid: Cid,
    /// Set semantics: strictly ascending by byte order (canonical).
    pub wasm_module_cids: Vec<Cid>,
    /// Set semantics: strictly ascending by byte order (canonical).
    pub asset_cids: Vec<Cid>,
    /// Execution order (semantic); duplicates rejected.
    pub migration_cids: Vec<Cid>,
    pub governance_change: Option<GovernanceChange>,
    pub test_results_cid: Cid,
    /// Epoch boundary at which the patch activates; never mid-simulation.
    pub activation_epoch: u64,
    pub rollback_plan: RollbackPlan,
}

fn u16_le(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes.try_into().unwrap())
}

fn u64_le(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().unwrap())
}

fn take(bytes: &[u8], n: usize) -> Option<(&[u8], &[u8])> {
    (bytes.len() >= n).then(|| bytes.split_at(n))
}

/// Append `count (u16 LE) || count x 32-byte cids`, asserting the list is
/// canonical for its semantics (encoders must not produce bytes decoders
/// would reject).
fn encode_cids(cids: &[Cid], ascending: bool, out: &mut Vec<u8>) {
    assert!(cids.len() <= MAX_CIDS_PER_LIST, "cid list over cap");
    assert!(
        cids_canonical(cids, ascending),
        "cid list not canonical (unsorted or duplicated)"
    );
    out.extend_from_slice(&(cids.len() as u16).to_le_bytes());
    for cid in cids {
        out.extend_from_slice(&cid.0);
    }
}

/// Decode one cid list, rejecting counts over the cap and non-canonical
/// contents (unsorted when `ascending`, duplicates always).
fn decode_cids(bytes: &[u8], ascending: bool) -> Option<(Vec<Cid>, &[u8])> {
    let (count, rest) = take(bytes, 2)?;
    let count = u16_le(count) as usize;
    if count > MAX_CIDS_PER_LIST {
        return None;
    }
    let (raw, rest) = take(rest, count * 32)?;
    let cids: Vec<Cid> = raw
        .chunks_exact(32)
        .map(|c| Cid(c.try_into().unwrap()))
        .collect();
    cids_canonical(&cids, ascending).then_some((cids, rest))
}

/// Canonicality for a cid list: strictly ascending when the list is a set,
/// duplicate-free always (a migration listed twice is a bug or an attack).
fn cids_canonical(cids: &[Cid], ascending: bool) -> bool {
    if ascending {
        cids.windows(2).all(|w| w[0] < w[1])
    } else {
        // Execution-ordered list: order is semantic, only duplicates are
        // non-canonical. Linear scan is fine at <= MAX_CIDS_PER_LIST.
        cids.iter()
            .enumerate()
            .all(|(i, cid)| !cids[..i].contains(cid))
    }
}

impl Proposal {
    /// Append the canonical encoding of this proposal to `out`:
    ///
    /// `version (u8 = 1) || kind (u8) || author (32) || git_commit (20) ||
    /// source_bundle_cid (32) || build_manifest_cid (32) ||
    /// n_wasm (u16 LE) || wasm cids (n x 32, strictly ascending) ||
    /// n_asset (u16 LE) || asset cids (n x 32, strictly ascending) ||
    /// n_migration (u16 LE) || migration cids (n x 32, execution order) ||
    /// governance tag (u8: 0 none | 1 + rule_module_cid (32)) ||
    /// test_results_cid (32) || activation_epoch (u64 LE) ||
    /// rollback tag (u8: 0 snapshot | 1 + n (u16 LE) + n x 32 cids)`
    ///
    /// Exactly one encoding exists per proposal: set-semantics cid lists
    /// must be strictly ascending and every list duplicate-free, which
    /// this method asserts and [`Proposal::decode`] rejects.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(PROPOSAL_FORMAT_VERSION);
        out.push(self.kind.as_byte());
        out.extend_from_slice(&self.author_public_key.0);
        out.extend_from_slice(&self.git_commit_hash);
        out.extend_from_slice(&self.source_bundle_cid.0);
        out.extend_from_slice(&self.build_manifest_cid.0);
        encode_cids(&self.wasm_module_cids, true, out);
        encode_cids(&self.asset_cids, true, out);
        encode_cids(&self.migration_cids, false, out);
        match &self.governance_change {
            None => out.push(0),
            Some(change) => {
                out.push(1);
                out.extend_from_slice(&change.rule_module_cid.0);
            }
        }
        out.extend_from_slice(&self.test_results_cid.0);
        out.extend_from_slice(&self.activation_epoch.to_le_bytes());
        match &self.rollback_plan {
            RollbackPlan::RevertToLastSignedSnapshot => out.push(0),
            RollbackPlan::ReverseMigrations {
                reverse_migration_cids,
            } => {
                out.push(1);
                encode_cids(reverse_migration_cids, false, out);
            }
        }
    }

    /// Decode one proposal from the front of `bytes`, returning it and the
    /// remaining bytes.
    ///
    /// Returns `None` for an unknown version, unknown tags, truncation,
    /// list counts over [`MAX_CIDS_PER_LIST`], or non-canonical cid lists.
    /// Decoding checks structure only — call [`crate::SignedProposal::verify`]
    /// and [`Proposal::validate`] before trusting the result.
    pub fn decode(bytes: &[u8]) -> Option<(Proposal, &[u8])> {
        let (version, rest) = take(bytes, 1)?;
        if version[0] != PROPOSAL_FORMAT_VERSION {
            return None;
        }
        let (kind, rest) = take(rest, 1)?;
        let kind = ProposalKind::from_byte(kind[0])?;
        let (author, rest) = take(rest, 32)?;
        let (git_commit, rest) = take(rest, 20)?;
        let (source_bundle, rest) = take(rest, 32)?;
        let (build_manifest, rest) = take(rest, 32)?;
        let (wasm_module_cids, rest) = decode_cids(rest, true)?;
        let (asset_cids, rest) = decode_cids(rest, true)?;
        let (migration_cids, rest) = decode_cids(rest, false)?;
        let (governance_tag, rest) = take(rest, 1)?;
        let (governance_change, rest) = match governance_tag[0] {
            0 => (None, rest),
            1 => {
                let (cid, rest) = take(rest, 32)?;
                (
                    Some(GovernanceChange {
                        rule_module_cid: Cid(cid.try_into().unwrap()),
                    }),
                    rest,
                )
            }
            _ => return None,
        };
        let (test_results, rest) = take(rest, 32)?;
        let (epoch, rest) = take(rest, 8)?;
        let (rollback_tag, rest) = take(rest, 1)?;
        let (rollback_plan, rest) = match rollback_tag[0] {
            0 => (RollbackPlan::RevertToLastSignedSnapshot, rest),
            1 => {
                let (reverse_migration_cids, rest) = decode_cids(rest, false)?;
                (
                    RollbackPlan::ReverseMigrations {
                        reverse_migration_cids,
                    },
                    rest,
                )
            }
            _ => return None,
        };
        Some((
            Proposal {
                kind,
                author_public_key: PlayerId(author.try_into().unwrap()),
                git_commit_hash: git_commit.try_into().unwrap(),
                source_bundle_cid: Cid(source_bundle.try_into().unwrap()),
                build_manifest_cid: Cid(build_manifest.try_into().unwrap()),
                wasm_module_cids,
                asset_cids,
                migration_cids,
                governance_change,
                test_results_cid: Cid(test_results.try_into().unwrap()),
                activation_epoch: u64_le(epoch),
                rollback_plan,
            },
            rest,
        ))
    }

    /// Decode exactly one proposal, rejecting trailing bytes.
    pub fn decode_exact(bytes: &[u8]) -> Option<Proposal> {
        match Self::decode(bytes)? {
            (proposal, []) => Some(proposal),
            _ => None,
        }
    }

    /// Every content id this proposal references, deduplicated and sorted
    /// ascending: the fetch list a peer resolves after the proposal is accepted.
    ///
    /// Covers all eight cid sources — source bundle, build manifest, wasm
    /// modules, assets, migrations, the governance rule module, test results,
    /// and any reverse-migration cids in the rollback plan. `git_commit_hash` is
    /// provenance, not content, so it is not fetched.
    pub fn referenced_cids(&self) -> Vec<Cid> {
        let mut cids: std::collections::BTreeSet<Cid> = std::collections::BTreeSet::new();
        cids.insert(self.source_bundle_cid);
        cids.insert(self.build_manifest_cid);
        cids.insert(self.test_results_cid);
        cids.extend(&self.wasm_module_cids);
        cids.extend(&self.asset_cids);
        cids.extend(&self.migration_cids);
        if let Some(change) = &self.governance_change {
            cids.insert(change.rule_module_cid);
        }
        if let RollbackPlan::ReverseMigrations {
            reverse_migration_cids,
        } = &self.rollback_plan
        {
            cids.extend(reverse_migration_cids);
        }
        cids.into_iter().collect()
    }

    /// The content-derived id of this proposal: SHA-256 over
    /// `PROPOSAL_ID_DOMAIN || encode()`.
    pub fn id(&self) -> ProposalId {
        let mut hasher = Sha256::new();
        hasher.update(PROPOSAL_ID_DOMAIN);
        let mut encoded = Vec::new();
        self.encode(&mut encoded);
        hasher.update(&encoded);
        ProposalId(hasher.finalize().into())
    }

    /// Check the manifest's internal consistency: the declared kind must
    /// match its contents where that is crisp, kernel changes are not
    /// hot-patchable in v1, and cid lists must be canonical (relevant for
    /// in-memory construction; [`Proposal::decode`] already rejects
    /// non-canonical bytes).
    pub fn validate(&self) -> Result<(), ValidationError> {
        let reverse_migration_cids = match &self.rollback_plan {
            RollbackPlan::RevertToLastSignedSnapshot => &[][..],
            RollbackPlan::ReverseMigrations {
                reverse_migration_cids,
            } => reverse_migration_cids,
        };
        for (cids, ascending) in [
            (&self.wasm_module_cids[..], true),
            (&self.asset_cids[..], true),
            (&self.migration_cids[..], false),
            (reverse_migration_cids, false),
        ] {
            if cids.len() > MAX_CIDS_PER_LIST || !cids_canonical(cids, ascending) {
                return Err(ValidationError::NonCanonical);
            }
        }
        if (self.kind == ProposalKind::Governance) != self.governance_change.is_some() {
            return Err(ValidationError::GovernanceChangeMismatch);
        }
        match self.kind {
            ProposalKind::Kernel => return Err(ValidationError::KernelChangeNotHotPatchable),
            ProposalKind::AssetPatch
                if !self.wasm_module_cids.is_empty() || !self.migration_cids.is_empty() =>
            {
                return Err(ValidationError::AssetPatchHasCode);
            }
            ProposalKind::GameplayCode if self.wasm_module_cids.is_empty() => {
                return Err(ValidationError::GameplayPatchWithoutCode);
            }
            _ => {}
        }
        if self.wasm_module_cids.is_empty()
            && self.asset_cids.is_empty()
            && self.migration_cids.is_empty()
            && self.governance_change.is_none()
        {
            return Err(ValidationError::EmptyProposal);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ValidationError {
    /// `kind == Governance` must hold exactly when `governance_change` is
    /// present.
    GovernanceChangeMismatch,
    /// An asset-only patch must not ship wasm modules or migrations.
    AssetPatchHasCode,
    /// A gameplay code patch must ship at least one wasm module.
    GameplayPatchWithoutCode,
    /// Kernel changes are not in-game hot patches in v1.
    KernelChangeNotHotPatchable,
    /// The proposal changes nothing: no content ids, no governance change.
    EmptyProposal,
    /// A cid list is unsorted, duplicated, or over [`MAX_CIDS_PER_LIST`].
    NonCanonical,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::GovernanceChangeMismatch => {
                write!(f, "governance kind and governance_change field disagree")
            }
            ValidationError::AssetPatchHasCode => {
                write!(f, "asset-only patch ships wasm modules or migrations")
            }
            ValidationError::GameplayPatchWithoutCode => {
                write!(f, "gameplay code patch ships no wasm modules")
            }
            ValidationError::KernelChangeNotHotPatchable => {
                write!(f, "kernel changes are not in-game hot patches in v1")
            }
            ValidationError::EmptyProposal => write!(f, "proposal changes nothing"),
            ValidationError::NonCanonical => {
                write!(f, "cid list unsorted, duplicated, or over cap")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referenced_cids_dedups_covers_all_sources_and_sorts() {
        // A shared cid appears in two lists; the governance rule and reverse
        // migrations are both present. `test_results_cid` deliberately equals a
        // wasm cid to prove cross-source dedup.
        let shared = Cid([7; 32]);
        let proposal = Proposal {
            kind: ProposalKind::Governance,
            author_public_key: PlayerId([0; 32]),
            git_commit_hash: [0xAB; 20],
            source_bundle_cid: Cid([1; 32]),
            build_manifest_cid: Cid([2; 32]),
            wasm_module_cids: vec![Cid([3; 32]), shared],
            asset_cids: vec![Cid([4; 32]), shared],
            migration_cids: vec![Cid([5; 32])],
            governance_change: Some(GovernanceChange {
                rule_module_cid: Cid([6; 32]),
            }),
            test_results_cid: shared,
            activation_epoch: 0,
            rollback_plan: RollbackPlan::ReverseMigrations {
                reverse_migration_cids: vec![Cid([8; 32])],
            },
        };

        let cids = proposal.referenced_cids();
        // git_commit_hash is not a cid, so 8 distinct cids: 1..=8.
        let expected: Vec<Cid> = (1u8..=8).map(|b| Cid([b; 32])).collect();
        assert_eq!(cids, expected);
        // Ascending and duplicate-free.
        assert!(cids.windows(2).all(|w| w[0] < w[1]));
    }
}
