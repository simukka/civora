//! Client-side epochs and the persisted accepted-proposal ledger.
//!
//! [`EpochClock`] turns wall-clock time into voting epochs (see
//! [`civora_governance::epoch_at`]); a proposal's window is open until
//! `now_epoch >= activation_epoch`. [`LedgerStore`] wraps the on-disk
//! [`Ledger`], persisting on every accepted entry. Certificate assembly at
//! window close and the certificate-handling helper [`apply_certificate`] live
//! here too, alongside the window evaluator system in
//! [`crate::ledger::evaluate_voting_windows`].

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use civora_governance::{
    EPOCH_SECS, Ledger, LedgerEntry, LedgerError, LedgerFileError, Proposal, SignedCertificate,
    epoch_at,
};

use crate::AppState;
use crate::identity::LocalIdentity;
use crate::net::{NetChannels, PeerRoster};
use crate::packs::{ContentStore, PackTracker, track_pack};
use crate::voting::{ProposalStatus, ProposalStore};

/// Dev knob: override the voting-window length in seconds. Must be set
/// identically on every peer — certificate verification is clock-free, so a
/// mismatch only skews the countdown UX, never validity.
pub const EPOCH_SECS_ENV: &str = "CIVORA_EPOCH_SECS";

/// Override the ledger file location (like `--ledger-file`).
pub const LEDGER_FILE_ENV: &str = "CIVORA_LEDGER_FILE";

/// How often the window evaluator actually scans (once per second is plenty;
/// epochs are seconds-granular at their finest).
const EVAL_INTERVAL_SECS: f32 = 1.0;

pub struct LedgerPlugin;

impl Plugin for LedgerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            evaluate_voting_windows.run_if(in_state(AppState::InGame)),
        )
        .add_systems(OnEnter(AppState::InGame), seed_accepted_from_ledger);
    }
}

/// Wall-clock voting clock. Reads the system time; the window length honors the
/// [`EPOCH_SECS_ENV`] dev override, defaulting to [`EPOCH_SECS`].
#[derive(Resource)]
pub struct EpochClock {
    pub epoch_secs: u64,
}

impl EpochClock {
    /// Build from the environment: `CIVORA_EPOCH_SECS` if set to a positive
    /// integer, else the default window.
    pub fn from_env() -> Self {
        let epoch_secs = std::env::var(EPOCH_SECS_ENV)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&s| s > 0)
            .unwrap_or(EPOCH_SECS);
        Self { epoch_secs }
    }

    /// Seconds since the Unix epoch (0 if the clock is before it).
    pub fn now_unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// The current voting epoch.
    pub fn now_epoch(&self) -> u64 {
        epoch_at(self.now_unix(), self.epoch_secs)
    }
}

/// The persisted accepted-proposal ledger and the path it lives at.
#[derive(Resource)]
pub struct LedgerStore {
    pub ledger: Ledger,
    pub path: PathBuf,
}

impl LedgerStore {
    /// Append `entry` through the ledger gate and, if it was new, persist the
    /// whole ledger (temp file + atomic rename). Returns whether it was added.
    pub fn append_and_save(&mut self, entry: LedgerEntry) -> Result<bool, LedgerStoreError> {
        let added = self
            .ledger
            .append(entry)
            .map_err(LedgerStoreError::Append)?;
        if added {
            self.ledger
                .save(&self.path)
                .map_err(LedgerStoreError::Save)?;
        }
        Ok(added)
    }
}

/// Why [`LedgerStore::append_and_save`] failed.
#[derive(Debug)]
pub enum LedgerStoreError {
    /// The entry failed the ledger gate (bad signature, invalid proposal, or a
    /// certificate that does not verify).
    Append(LedgerError),
    /// The entry verified but the ledger could not be written to disk.
    Save(LedgerFileError),
}

impl std::fmt::Display for LedgerStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LedgerStoreError::Append(err) => write!(f, "{err}"),
            LedgerStoreError::Save(err) => write!(f, "could not persist ledger: {err}"),
        }
    }
}

impl std::error::Error for LedgerStoreError {}

/// Resolve the ledger file path: the `--ledger-file` override, else
/// `CIVORA_LEDGER_FILE`, else `<config dir>/civora/genesis-0.ledger`. Mirrors
/// the identity key path so two instances on one machine can keep distinct
/// ledgers the same way they keep distinct keys.
pub fn ledger_path(overridden: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = overridden.or_else(|| std::env::var_os(LEDGER_FILE_ENV).map(PathBuf::from))
    {
        return Ok(path);
    }
    dirs::config_dir()
        .map(|dir| dir.join("civora").join("genesis-0.ledger"))
        .ok_or_else(|| "no OS config directory found for the ledger".into())
}

/// Apply a finality certificate: pair it with its proposal (from the store),
/// append the entry through the ledger gate (that append *is* the
/// verification), and on success mark the proposal Accepted. A certificate
/// whose proposal has not arrived yet is parked in the store until it does.
///
/// Returns the accepted [`Proposal`] when the certificate finalized it (whether
/// newly appended or already known), so the caller can track its patch pack;
/// `None` when the certificate was parked or rejected.
pub fn apply_certificate(
    store: &mut ProposalStore,
    ledger: &mut LedgerStore,
    certificate: SignedCertificate,
) -> Option<Proposal> {
    let id = certificate.certificate.proposal_id;
    let Some(signed) = store.signed_proposal(&id) else {
        store.park_certificate(certificate);
        return None;
    };
    let proposal = signed.proposal.clone();
    let entry = LedgerEntry {
        proposal: signed,
        certificate,
    };
    match ledger.append_and_save(entry) {
        Ok(added) => {
            // Any valid certificate flips the proposal to Accepted, even over a
            // local Rejected (the certifier's roster differed from ours).
            store.set_status(&id, ProposalStatus::Accepted);
            if added {
                info!("proposal {} accepted", id.short());
            } else {
                debug!("proposal {} already accepted", id.short());
            }
            Some(proposal)
        }
        Err(err) => {
            debug!("dropped certificate for {}: {err}", id.short());
            None
        }
    }
}

/// Seed the store and pack tracker from the persisted ledger on entering the
/// game, so accepted history is visible immediately and its content resolves
/// (before any gossip).
fn seed_accepted_from_ledger(
    mut store: ResMut<ProposalStore>,
    mut tracker: ResMut<PackTracker>,
    ledger: Res<LedgerStore>,
    content: Res<ContentStore>,
    channels: Option<Res<NetChannels>>,
) {
    for entry in ledger.ledger.entries() {
        store.insert_accepted(entry.proposal.clone());
        track_pack(
            &mut tracker,
            &content.0,
            channels.as_deref(),
            entry.proposal.proposal_id(),
            &entry.proposal.proposal,
        );
    }
}

/// Once per second, close every open proposal whose voting window has passed:
/// assemble and gossip a certificate if the roster-filtered tally clears
/// quorum (status Accepted), else mark Rejected (ballots cast) or Expired (no
/// ballots).
#[allow(clippy::too_many_arguments)]
fn evaluate_voting_windows(
    mut store: ResMut<ProposalStore>,
    mut ledger: ResMut<LedgerStore>,
    mut tracker: ResMut<PackTracker>,
    content: Res<ContentStore>,
    clock: Res<EpochClock>,
    roster: Res<PeerRoster>,
    local: Res<LocalIdentity>,
    channels: Option<Res<NetChannels>>,
    time: Res<Time>,
    mut since_scan: Local<f32>,
) {
    *since_scan += time.delta_secs();
    if *since_scan < EVAL_INTERVAL_SECS {
        return;
    }
    *since_scan = 0.0;

    let now_epoch = clock.now_epoch();
    let closable = store.closable_proposals(now_epoch);
    for id in closable {
        if ledger.ledger.contains(&id) {
            store.set_status(&id, ProposalStatus::Accepted);
            continue;
        }
        let Some(entry) = store.get(&id) else {
            continue;
        };
        let proposal = entry.signed.proposal.clone();
        let ballots = entry.ballots();

        // Roster = connected peers plus ourselves, canonicalized.
        let mut ids: Vec<_> = roster.0.iter().map(|(pid, _)| *pid).collect();
        ids.push(local.identity.player_id());
        ids.sort();
        ids.dedup();

        let cert = SignedCertificate::certify(
            &local.identity,
            &proposal,
            &ids,
            &ballots,
            ledger.ledger.rule_version(),
            now_epoch,
        );
        match cert {
            Some(cert) => {
                let signed = entry.signed.clone();
                match ledger.append_and_save(LedgerEntry {
                    proposal: signed,
                    certificate: cert.clone(),
                }) {
                    Ok(_) => {
                        store.set_status(&id, ProposalStatus::Accepted);
                        if let Some(channels) = &channels {
                            let _ = channels
                                .commands
                                .send(civora_net::NetCommand::PublishCertificate(Box::new(cert)));
                        }
                        // Resolve the accepted proposal's content (local already
                        // for our own; a fetch for anything we lack).
                        track_pack(&mut tracker, &content.0, channels.as_deref(), id, &proposal);
                        info!("proposal {} accepted at epoch {now_epoch}", id.short());
                    }
                    Err(err) => warn!("could not accept {}: {err}", id.short()),
                }
            }
            None => {
                // Quorum failed: rejected if anyone voted, expired if nobody did.
                let (yes, no) = entry.tally();
                let status = if yes + no > 0 {
                    ProposalStatus::Rejected
                } else {
                    ProposalStatus::Expired
                };
                store.set_status(&id, status);
                info!("proposal {} {:?} at epoch {now_epoch}", id.short(), status);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use civora_governance::{
        Cid, Proposal, ProposalId, ProposalKind, RollbackPlan, SignedProposal, SignedVote, Vote,
        VoteChoice,
    };
    use civora_identity::{Identity, PlayerId};
    use std::collections::BTreeMap;

    fn identity(seed: u8) -> Identity {
        Identity::from_seed([seed; 32])
    }

    fn proposal(author: &Identity) -> Proposal {
        Proposal {
            kind: ProposalKind::AssetPatch,
            author_public_key: author.player_id(),
            git_commit_hash: [0x55; 20],
            source_bundle_cid: Cid([1; 32]),
            build_manifest_cid: Cid([2; 32]),
            wasm_module_cids: vec![],
            asset_cids: vec![Cid([3; 32])],
            migration_cids: vec![],
            governance_change: None,
            test_results_cid: Cid([4; 32]),
            activation_epoch: 0,
            rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
        }
    }

    fn solo_certificate(
        author: &Identity,
        id: ProposalId,
        proposal: &Proposal,
    ) -> SignedCertificate {
        let ballots: BTreeMap<PlayerId, SignedVote> = [(
            author.player_id(),
            SignedVote::sign(
                author,
                Vote {
                    proposal_id: id,
                    voter: author.player_id(),
                    choice: VoteChoice::Yes,
                },
            ),
        )]
        .into();
        SignedCertificate::certify(author, proposal, &[author.player_id()], &ballots, 1, 0).unwrap()
    }

    fn store_at(dir: &std::path::Path) -> LedgerStore {
        LedgerStore {
            ledger: Ledger::default(),
            path: dir.join("genesis-0.ledger"),
        }
    }

    #[test]
    fn certificate_overrides_a_local_rejection() {
        let dir = tempfile::tempdir().unwrap();
        let author = identity(1);
        let proposal = proposal(&author);
        let signed = SignedProposal::sign(&author, proposal.clone());
        let id = signed.proposal_id();

        let mut store = ProposalStore::default();
        store.insert_proposal(signed).unwrap();
        // We locally rejected it (our roster differed from the certifier's).
        store.set_status(&id, ProposalStatus::Rejected);

        let mut ledger = store_at(dir.path());
        let cert = solo_certificate(&author, id, &proposal);
        assert!(apply_certificate(&mut store, &mut ledger, cert).is_some());

        assert_eq!(store.get(&id).unwrap().status, ProposalStatus::Accepted);
        assert!(ledger.ledger.contains(&id));
        // Persisted: a fresh load sees the accepted entry.
        assert_eq!(Ledger::load(&ledger.path).unwrap().len(), 1);
    }

    #[test]
    fn certificate_before_its_proposal_is_parked_then_applied() {
        let dir = tempfile::tempdir().unwrap();
        let author = identity(1);
        let proposal = proposal(&author);
        let signed = SignedProposal::sign(&author, proposal.clone());
        let id = signed.proposal_id();

        let mut store = ProposalStore::default();
        let mut ledger = store_at(dir.path());

        // Certificate arrives first: parked, nothing accepted yet.
        let cert = solo_certificate(&author, id, &proposal);
        assert!(apply_certificate(&mut store, &mut ledger, cert).is_none());
        assert!(!ledger.ledger.contains(&id));

        // Proposal arrives; draining the parked certificate accepts it.
        store.insert_proposal(signed).unwrap();
        let parked = store
            .take_pending_certificate(&id)
            .expect("cert was parked");
        assert!(apply_certificate(&mut store, &mut ledger, parked).is_some());
        assert_eq!(store.get(&id).unwrap().status, ProposalStatus::Accepted);
        assert!(ledger.ledger.contains(&id));
    }
}
