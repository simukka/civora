//! Content-addressed patch packs: the local blob store and the per-proposal
//! fetch tracker.
//!
//! When a proposal is accepted, its manifest's referenced artifacts must land —
//! hash-verified — in every peer's local [`ContentStore`]. [`track_pack`] is the
//! single choke point every accept path calls: it splits a proposal's
//! [`referenced_cids`](civora_governance::Proposal::referenced_cids) into what
//! is already local and what is missing, and fires a [`NetCommand::FetchBlob`]
//! per missing cid. [`PackTracker`] remembers per-proposal progress; a 10 s
//! [`retry_missing_blobs`] timer re-requests anything still missing while peers
//! exist. Nothing here loads or executes content — it lands on disk and stops.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use bevy::prelude::*;
use civora_governance::{BlobStore, Cid, Proposal, ProposalId};
use civora_net::NetCommand;

use crate::AppState;
use crate::net::{NetChannels, PeerRoster};

/// Override the content-store directory (like `--store-dir`).
pub const STORE_DIR_ENV: &str = "CIVORA_STORE_DIR";

/// How often to re-request blobs still missing after acceptance.
const RETRY_FETCH_SECS: f32 = 10.0;

/// Cap on per-blob rows rendered in a proposal's detail view.
pub const MAX_DETAIL_BLOB_ROWS: usize = 8;

pub struct PacksPlugin;

impl Plugin for PacksPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PackTracker>().add_systems(
            Update,
            retry_missing_blobs
                .run_if(in_state(AppState::InGame))
                .run_if(resource_exists::<NetChannels>),
        );
    }
}

/// The local content-addressed blob store. Present in every session (an offline
/// F9 demo puts blobs into it before any networking exists).
#[derive(Resource)]
pub struct ContentStore(pub BlobStore);

/// Resolve the content-store directory: the `--store-dir` override, else
/// [`STORE_DIR_ENV`], else `<config dir>/civora/store`. Mirrors the identity key
/// and ledger paths so two instances on one machine keep distinct stores.
pub fn store_dir(overridden: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = overridden.or_else(|| std::env::var_os(STORE_DIR_ENV).map(PathBuf::from)) {
        return Ok(path);
    }
    dirs::config_dir()
        .map(|dir| dir.join("civora").join("store"))
        .ok_or_else(|| "no OS config directory found for the content store".into())
}

/// Per-blob resolution state within a pack, for the UI.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlobState {
    /// Present and hash-verified in the local store.
    Local,
    /// Not local yet; a fetch is in flight or queued.
    Fetching,
    /// The most recent fetch attempt failed; the retry timer will try again.
    Failed,
}

impl BlobState {
    pub fn tag(self) -> &'static str {
        match self {
            BlobState::Local => "local",
            BlobState::Fetching => "fetching",
            BlobState::Failed => "failed",
        }
    }
}

/// Fetch progress for one accepted proposal's pack.
pub struct PackStatus {
    /// Every referenced cid, ascending (from `referenced_cids`).
    pub cids: Vec<Cid>,
    /// Cids not yet in the local store.
    missing: BTreeSet<Cid>,
    /// Missing cids with a fetch currently in flight.
    in_flight: BTreeSet<Cid>,
    /// Missing cids whose most recent attempt failed, with the reason.
    failed: BTreeMap<Cid, String>,
}

impl PackStatus {
    /// Total referenced blobs.
    pub fn total(&self) -> usize {
        self.cids.len()
    }

    /// Blobs already local (verified in the store).
    pub fn local(&self) -> usize {
        self.cids.len().saturating_sub(self.missing.len())
    }

    /// Whether every referenced blob is local.
    pub fn complete(&self) -> bool {
        self.missing.is_empty()
    }

    /// Missing blobs that are fetching or queued (i.e. not currently failed).
    pub fn fetching(&self) -> usize {
        self.missing.len().saturating_sub(self.failed.len())
    }

    /// Missing blobs whose last attempt failed.
    pub fn failed(&self) -> usize {
        self.failed.len()
    }

    /// The display state of one referenced cid.
    pub fn state_of(&self, cid: &Cid) -> BlobState {
        if !self.missing.contains(cid) {
            BlobState::Local
        } else if self.failed.contains_key(cid) {
            BlobState::Failed
        } else {
            BlobState::Fetching
        }
    }
}

/// Accepted-proposal packs and their fetch progress.
#[derive(Resource, Default)]
pub struct PackTracker {
    packs: BTreeMap<ProposalId, PackStatus>,
}

impl PackTracker {
    pub fn is_empty(&self) -> bool {
        self.packs.is_empty()
    }

    pub fn get(&self, id: &ProposalId) -> Option<&PackStatus> {
        self.packs.get(id)
    }

    /// `(complete, syncing)` pack counts for the HUD.
    pub fn counts(&self) -> (usize, usize) {
        let complete = self.packs.values().filter(|p| p.complete()).count();
        (complete, self.packs.len() - complete)
    }

    /// Register (or refresh) a pack for `id`, recomputing which cids are missing
    /// against the store. Idempotent: re-tracking an accepted proposal drops
    /// now-local cids and preserves in-flight/failed state for those still gone.
    fn register(&mut self, id: ProposalId, cids: Vec<Cid>, store: &BlobStore) {
        let missing: BTreeSet<Cid> = cids.iter().copied().filter(|c| !store.has(c)).collect();
        let status = self.packs.entry(id).or_insert_with(|| PackStatus {
            cids: Vec::new(),
            missing: BTreeSet::new(),
            in_flight: BTreeSet::new(),
            failed: BTreeMap::new(),
        });
        status.cids = cids;
        status.in_flight.retain(|c| missing.contains(c));
        status.failed.retain(|c, _| missing.contains(c));
        status.missing = missing;
    }

    /// Missing cids not already in flight, marked in flight and cleared of any
    /// prior failure — the (re)fetch set shared by [`track_pack`] and the retry
    /// timer. Deduplicated across packs that share a cid.
    fn take_eligible(&mut self) -> Vec<Cid> {
        let mut fetch: BTreeSet<Cid> = BTreeSet::new();
        for status in self.packs.values_mut() {
            for cid in status.missing.iter().copied().collect::<Vec<_>>() {
                if status.in_flight.insert(cid) {
                    status.failed.remove(&cid);
                    fetch.insert(cid);
                }
            }
        }
        fetch.into_iter().collect()
    }

    /// A blob arrived and was stored: clear it from every pack that wanted it.
    pub fn on_fetched(&mut self, cid: &Cid) {
        for status in self.packs.values_mut() {
            status.missing.remove(cid);
            status.in_flight.remove(cid);
            status.failed.remove(cid);
        }
    }

    /// A fetch failed: drop it from in-flight and record the reason so the retry
    /// timer picks it up again.
    pub fn on_failed(&mut self, cid: &Cid, reason: &str) {
        for status in self.packs.values_mut() {
            if status.missing.contains(cid) {
                status.in_flight.remove(cid);
                status.failed.insert(*cid, reason.to_owned());
            }
        }
    }

    /// Forget all in-flight state (e.g. the network thread died); the retry
    /// timer will re-request from scratch once peers return.
    pub fn clear_in_flight(&mut self) {
        for status in self.packs.values_mut() {
            status.in_flight.clear();
        }
    }
}

/// Track a newly accepted proposal's pack and fire fetches for its missing
/// blobs. The single accept-time choke point: local certification, remote
/// certificates (join-synced included), and startup ledger seeding all call it.
pub fn track_pack(
    tracker: &mut PackTracker,
    store: &BlobStore,
    channels: Option<&NetChannels>,
    id: ProposalId,
    proposal: &Proposal,
) {
    tracker.register(id, proposal.referenced_cids(), store);
    request_missing(tracker, channels);
}

/// Send a [`NetCommand::FetchBlob`] for every eligible missing cid. No-op
/// offline (no channels): the blobs a solo player needs are already local.
fn request_missing(tracker: &mut PackTracker, channels: Option<&NetChannels>) {
    let Some(channels) = channels else {
        return;
    };
    for cid in tracker.take_eligible() {
        let _ = channels.commands.send(NetCommand::FetchBlob { cid });
    }
}

/// Every 10 s, re-request blobs still missing (including ones whose last fetch
/// failed) while peers exist. Skips entirely with no peers connected.
fn retry_missing_blobs(
    mut tracker: ResMut<PackTracker>,
    channels: Res<NetChannels>,
    roster: Res<PeerRoster>,
    time: Res<Time>,
    mut since_retry: Local<f32>,
) {
    *since_retry += time.delta_secs();
    if *since_retry < RETRY_FETCH_SECS {
        return;
    }
    *since_retry = 0.0;
    if roster.0.is_empty() {
        return;
    }
    request_missing(&mut tracker, Some(&channels));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_store() -> (BlobStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (BlobStore::open(dir.path().join("blobs")).unwrap(), dir)
    }

    fn proposal_with(assets: &[&[u8]], store: &BlobStore) -> Proposal {
        use civora_governance::{ProposalKind, RollbackPlan};
        // Put the "source/build/tests" trio plus the given assets, so referenced
        // cids resolve to real stored blobs.
        let cid = |bytes: &[u8]| store.put(bytes).unwrap();
        let mut asset_cids: Vec<Cid> = assets.iter().map(|a| cid(a)).collect();
        asset_cids.sort();
        asset_cids.dedup();
        Proposal {
            kind: ProposalKind::AssetPatch,
            author_public_key: civora_identity::PlayerId([0; 32]),
            git_commit_hash: [0; 20],
            source_bundle_cid: cid(b"source"),
            build_manifest_cid: cid(b"build"),
            wasm_module_cids: vec![],
            asset_cids,
            migration_cids: vec![],
            governance_change: None,
            test_results_cid: cid(b"tests"),
            activation_epoch: 0,
            rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
        }
    }

    #[test]
    fn a_fully_local_pack_is_complete_immediately() {
        let (store, _dir) = new_store();
        let proposal = proposal_with(&[b"asset-a", b"asset-b"], &store);
        let id = proposal.id();

        let mut tracker = PackTracker::default();
        // No channels (offline): nothing to fetch, and everything is local.
        track_pack(&mut tracker, &store, None, id, &proposal);

        let status = tracker.get(&id).unwrap();
        assert!(status.complete());
        assert_eq!(status.local(), status.total());
        assert_eq!(tracker.counts(), (1, 0));
        for cid in &status.cids {
            assert_eq!(status.state_of(cid), BlobState::Local);
        }
    }

    #[test]
    fn missing_blobs_drive_fetch_state_and_clear_on_arrival() {
        let (store, _dir) = new_store();
        // Build a proposal referencing an asset, then wipe the store so every
        // cid is missing.
        let proposal = proposal_with(&[b"asset-a"], &store);
        let id = proposal.id();
        let (empty, _dir2) = new_store();

        let mut tracker = PackTracker::default();
        track_pack(&mut tracker, &empty, None, id, &proposal);
        let status = tracker.get(&id).unwrap();
        assert!(!status.complete());
        assert_eq!(status.local(), 0);
        assert_eq!(tracker.counts(), (0, 1));
        let a_cid = proposal.source_bundle_cid;
        assert_eq!(status.state_of(&a_cid), BlobState::Fetching);

        // A failure flips the cid to Failed, then an arrival clears it.
        tracker.on_failed(&a_cid, "not found");
        assert_eq!(
            tracker.get(&id).unwrap().state_of(&a_cid),
            BlobState::Failed
        );
        tracker.on_fetched(&a_cid);
        assert_eq!(tracker.get(&id).unwrap().state_of(&a_cid), BlobState::Local);
    }
}
