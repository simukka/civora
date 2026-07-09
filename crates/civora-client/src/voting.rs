//! The voting UI: the in-game proposal store, the keyboard-driven proposal
//! panel, and vote casting.
//!
//! Everyone sees the open-proposal count on the HUD; `P` opens the list,
//! Up/Down + Enter select a proposal, and `Y`/`N` cast a signed ballot from
//! the detail view while its voting window is open. A proposal's window closes
//! at its `activation_epoch` ([`crate::ledger`] owns epochs, certificate
//! assembly, and the persisted ledger); after close the entry carries a stored
//! [`ProposalStatus`] — Accepted, Rejected, or Expired — and ballots go inert.

use std::collections::BTreeMap;
use std::fmt::Write;

use bevy::prelude::*;
use civora_governance::{
    Proposal, ProposalId, RollbackPlan, SignedCertificate, SignedProposal, SignedVote,
    ValidationError, Vote, VoteChoice,
};
use civora_identity::{PlayerId, VerifyError};
use civora_net::NetCommand;

use crate::AppState;
use crate::identity::LocalIdentity;
use crate::ledger::{EpochClock, LedgerStore};
use crate::net::NetChannels;

/// Cap on votes held for proposals we have not seen yet (gossip can deliver
/// a vote before its proposal). Guards memory only.
const MAX_PENDING_VOTES: usize = 1024;

/// Cap on certificates held for proposals we have not seen yet (replace-by
/// proposal-id, so at most one per proposal). Guards memory only.
const MAX_PENDING_CERTS: usize = 64;

/// Lifecycle of a proposal in the store. Stored, not derived: the window
/// evaluator sets Rejected/Expired at close, and any valid certificate flips
/// it to Accepted (including over a local Rejected, if the certifier's roster
/// differed). Only `Open` proposals count toward the HUD.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProposalStatus {
    Open,
    Accepted,
    Rejected,
    Expired,
}

impl ProposalStatus {
    /// Lowercase tag for the UI: `[open]`, `[accepted]`, ...
    pub fn tag(self) -> &'static str {
        match self {
            ProposalStatus::Open => "open",
            ProposalStatus::Accepted => "accepted",
            ProposalStatus::Rejected => "rejected",
            ProposalStatus::Expired => "expired",
        }
    }
}

pub struct VotingPlugin;

impl Plugin for VotingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ProposalStore>()
            .init_resource::<VotingUi>()
            .add_systems(
                Update,
                (voting_input, sync_voting_panel, update_voting_panel_text)
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

/// Why a gossiped proposal or vote was not admitted to the store.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StoreError {
    BadSignature(VerifyError),
    InvalidProposal(ValidationError),
    /// The target proposal's voting window has closed (or it is no longer
    /// Open), so no further ballots count.
    VotingClosed,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::BadSignature(err) => write!(f, "bad signature: {err}"),
            StoreError::InvalidProposal(err) => write!(f, "invalid manifest: {err}"),
            StoreError::VotingClosed => write!(f, "voting window closed"),
        }
    }
}

/// One proposal, the signed ballots received for it, and its lifecycle status.
pub struct ProposalEntry {
    pub signed: SignedProposal,
    /// Latest signed ballot per voter: a revote replaces the earlier one. The
    /// full [`SignedVote`] is kept (not just the choice) because certificate
    /// assembly at window close needs the signatures — the "binding votes to
    /// an ordering" milestone 5 promised.
    pub votes: BTreeMap<PlayerId, SignedVote>,
    pub status: ProposalStatus,
}

/// Every proposal and vote this session has seen and verified. The gate for
/// all inserts, local and remote: signatures must verify (and manifests
/// validate) or the message is dropped.
#[derive(Resource, Default)]
pub struct ProposalStore {
    proposals: BTreeMap<ProposalId, ProposalEntry>,
    /// Verified votes whose proposal has not arrived yet, drained into the
    /// entry when it does.
    pending_votes: Vec<SignedVote>,
    /// Verified certificates whose proposal has not arrived yet, drained when
    /// it does (replace-by-proposal-id, so at most one per proposal).
    pending_certs: Vec<SignedCertificate>,
}

impl ProposalStore {
    /// Admit a proposal: verify its signature, validate the manifest, then
    /// insert as [`ProposalStatus::Open`]. The window evaluator closes
    /// past-window entries on its next scan. Returns whether it was new
    /// (redelivery is normal for gossip).
    pub fn insert_proposal(&mut self, signed: SignedProposal) -> Result<bool, StoreError> {
        signed.verify().map_err(StoreError::BadSignature)?;
        signed
            .proposal
            .validate()
            .map_err(StoreError::InvalidProposal)?;
        let id = signed.proposal_id();
        if self.proposals.contains_key(&id) {
            return Ok(false);
        }
        let votes = self.drain_pending_votes(&id);
        self.proposals.insert(
            id,
            ProposalEntry {
                signed,
                votes,
                status: ProposalStatus::Open,
            },
        );
        Ok(true)
    }

    /// Seed a proposal known to be accepted (from a ledger entry). Verifies and
    /// validates defensively, then forces [`ProposalStatus::Accepted`] — even
    /// if the proposal was already present under a different status.
    pub fn insert_accepted(&mut self, signed: SignedProposal) {
        if signed.verify().is_err() || signed.proposal.validate().is_err() {
            return;
        }
        let id = signed.proposal_id();
        match self.proposals.get_mut(&id) {
            Some(entry) => entry.status = ProposalStatus::Accepted,
            None => {
                let votes = self.drain_pending_votes(&id);
                self.proposals.insert(
                    id,
                    ProposalEntry {
                        signed,
                        votes,
                        status: ProposalStatus::Accepted,
                    },
                );
            }
        }
    }

    fn drain_pending_votes(&mut self, id: &ProposalId) -> BTreeMap<PlayerId, SignedVote> {
        let mut votes = BTreeMap::new();
        self.pending_votes.retain(|pending| {
            if pending.vote.proposal_id == *id {
                votes.insert(pending.vote.voter, *pending);
                false
            } else {
                true
            }
        });
        votes
    }

    /// Admit a ballot: verify its signature, then tally it (latest ballot per
    /// voter wins). A vote for a closed or non-open proposal is rejected
    /// ([`StoreError::VotingClosed`]); a vote for an unseen proposal is parked
    /// until the proposal arrives.
    pub fn insert_vote(&mut self, signed: SignedVote, now_epoch: u64) -> Result<(), StoreError> {
        signed.verify().map_err(StoreError::BadSignature)?;
        match self.proposals.get_mut(&signed.vote.proposal_id) {
            Some(entry) => {
                if entry.status != ProposalStatus::Open
                    || now_epoch >= entry.signed.proposal.activation_epoch
                {
                    return Err(StoreError::VotingClosed);
                }
                entry.votes.insert(signed.vote.voter, signed);
            }
            None => {
                self.pending_votes.retain(|pending| {
                    (pending.vote.proposal_id, pending.vote.voter)
                        != (signed.vote.proposal_id, signed.vote.voter)
                });
                if self.pending_votes.len() < MAX_PENDING_VOTES {
                    self.pending_votes.push(signed);
                }
            }
        }
        Ok(())
    }

    /// Park a certificate whose proposal has not arrived yet (replace-by-id).
    pub fn park_certificate(&mut self, certificate: SignedCertificate) {
        let id = certificate.certificate.proposal_id;
        self.pending_certs
            .retain(|c| c.certificate.proposal_id != id);
        if self.pending_certs.len() < MAX_PENDING_CERTS {
            self.pending_certs.push(certificate);
        }
    }

    /// Remove and return a parked certificate for `id`, if any.
    pub fn take_pending_certificate(&mut self, id: &ProposalId) -> Option<SignedCertificate> {
        let pos = self
            .pending_certs
            .iter()
            .position(|c| c.certificate.proposal_id == *id)?;
        Some(self.pending_certs.remove(pos))
    }

    /// A clone of the signed proposal for `id`, for building a ledger entry.
    pub fn signed_proposal(&self, id: &ProposalId) -> Option<SignedProposal> {
        self.proposals.get(id).map(|entry| entry.signed.clone())
    }

    /// Force a proposal's status (window close, or a certificate accepting it).
    pub fn set_status(&mut self, id: &ProposalId, status: ProposalStatus) {
        if let Some(entry) = self.proposals.get_mut(id) {
            entry.status = status;
        }
    }

    /// Ids of `Open` proposals whose voting window has closed
    /// (`now_epoch >= activation_epoch`).
    pub fn closable_proposals(&self, now_epoch: u64) -> Vec<ProposalId> {
        self.proposals
            .iter()
            .filter(|(_, entry)| {
                entry.status == ProposalStatus::Open
                    && now_epoch >= entry.signed.proposal.activation_epoch
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Count of `Open` proposals only (the HUD number).
    pub fn open_count(&self) -> usize {
        self.proposals
            .values()
            .filter(|entry| entry.status == ProposalStatus::Open)
            .count()
    }

    /// Total proposals of every status — the number of rows the list renders,
    /// and the bound the list cursor moves within.
    pub fn len(&self) -> usize {
        self.proposals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.proposals.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ProposalId, &ProposalEntry)> {
        self.proposals.iter()
    }

    pub fn get(&self, id: &ProposalId) -> Option<&ProposalEntry> {
        self.proposals.get(id)
    }
}

impl ProposalEntry {
    /// `(yes, no)` under latest-ballot-per-voter.
    pub fn tally(&self) -> (usize, usize) {
        let yes = self
            .votes
            .values()
            .filter(|sv| sv.vote.choice == VoteChoice::Yes)
            .count();
        (yes, self.votes.len() - yes)
    }

    /// The ballots keyed by voter, as certificate assembly consumes them.
    pub fn ballots(&self) -> BTreeMap<PlayerId, SignedVote> {
        self.votes.clone()
    }
}

/// Where the voting panel is: closed (HUD count only), the proposal list,
/// or one proposal's detail + ballot view.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub enum VotingUi {
    #[default]
    Closed,
    List {
        cursor: usize,
    },
    Detail {
        id: ProposalId,
    },
}

fn voting_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut ui: ResMut<VotingUi>,
    mut store: ResMut<ProposalStore>,
    local: Res<LocalIdentity>,
    clock: Res<EpochClock>,
    channels: Option<Res<NetChannels>>,
) {
    // P walks back out one level: detail -> list -> closed.
    if keys.just_pressed(KeyCode::KeyP) {
        *ui = match *ui {
            VotingUi::Closed => VotingUi::List { cursor: 0 },
            VotingUi::List { .. } => VotingUi::Closed,
            VotingUi::Detail { .. } => VotingUi::List { cursor: 0 },
        };
        return;
    }

    match *ui {
        VotingUi::Closed => {}
        VotingUi::List { cursor } => {
            // The list shows proposals of every status, so the cursor moves
            // over all rows — not just the Open ones the HUD counts.
            let last = store.len().saturating_sub(1);
            if keys.just_pressed(KeyCode::ArrowUp) {
                *ui = VotingUi::List {
                    cursor: cursor.saturating_sub(1),
                };
            }
            if keys.just_pressed(KeyCode::ArrowDown) {
                *ui = VotingUi::List {
                    cursor: (cursor + 1).min(last),
                };
            }
            if keys.just_pressed(KeyCode::Enter)
                && let Some((id, _)) = store.iter().nth(cursor.min(last))
            {
                *ui = VotingUi::Detail { id: *id };
            }
        }
        VotingUi::Detail { id } => {
            let choice = if keys.just_pressed(KeyCode::KeyY) {
                Some(VoteChoice::Yes)
            } else if keys.just_pressed(KeyCode::KeyN) {
                Some(VoteChoice::No)
            } else {
                None
            };
            // Y/N are inert once the window is closed (status != Open).
            let open = store
                .get(&id)
                .is_some_and(|entry| entry.status == ProposalStatus::Open);
            if let Some(choice) = choice
                && open
            {
                let signed = SignedVote::sign(
                    &local.identity,
                    Vote {
                        proposal_id: id,
                        voter: local.identity.player_id(),
                        choice,
                    },
                );
                match store.insert_vote(signed, clock.now_epoch()) {
                    Ok(()) => {
                        // Gossipsub does not loop back: our own store already
                        // counted it above.
                        if let Some(channels) = &channels {
                            let _ = channels.commands.send(NetCommand::PublishVote(signed));
                        }
                    }
                    // The window closed between the status check and now, or a
                    // duplicate — either way, nothing to gossip.
                    Err(err) => debug!("vote not cast: {err}"),
                }
            }
        }
    }
}

#[derive(Component)]
struct VotingPanelRoot;

#[derive(Component)]
struct VotingPanelText;

/// Spawn/despawn the panel when the UI opens or closes (house rule: UI that
/// isn't shown doesn't exist).
fn sync_voting_panel(
    mut commands: Commands,
    ui: Res<VotingUi>,
    panel: Query<Entity, With<VotingPanelRoot>>,
) {
    if !ui.is_changed() {
        return;
    }
    let open = *ui != VotingUi::Closed;
    match (open, panel.single().ok()) {
        (false, Some(entity)) => commands.entity(entity).despawn(),
        (true, None) => {
            commands
                .spawn((
                    VotingPanelRoot,
                    Node {
                        position_type: PositionType::Absolute,
                        top: px(64),
                        left: percent(50),
                        width: px(560),
                        margin: UiRect {
                            left: px(-280),
                            ..default()
                        },
                        padding: UiRect::all(px(12)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.10, 0.10, 0.15, 0.92)),
                    // Below the start menu (z 10) if states ever overlap.
                    GlobalZIndex(5),
                ))
                .with_children(|parent| {
                    parent.spawn((
                        VotingPanelText,
                        Text::default(),
                        TextFont {
                            font_size: FontSize::Px(14.0),
                            ..default()
                        },
                        TextColor(Color::WHITE),
                    ));
                });
        }
        _ => {}
    }
}

fn update_voting_panel_text(
    ui: Res<VotingUi>,
    store: Res<ProposalStore>,
    local: Res<LocalIdentity>,
    clock: Res<EpochClock>,
    ledger: Res<LedgerStore>,
    mut panel: Query<&mut Text, With<VotingPanelText>>,
) {
    let Ok(mut text) = panel.single_mut() else {
        return;
    };
    let text = &mut text.0;
    text.clear();

    // ASCII only: the default font has no em-dash glyph.
    match *ui {
        VotingUi::Closed => {}
        VotingUi::List { cursor } => write_list(text, &store, cursor),
        VotingUi::Detail { id } => match store.get(&id) {
            Some(entry) => write_detail(text, entry, local.identity.player_id(), &clock, &ledger),
            // The id vanished (cannot happen in this milestone: proposals
            // are never removed) — fall back to the list rendering.
            None => write_list(text, &store, 0),
        },
    }
}

fn write_list(text: &mut String, store: &ProposalStore, cursor: usize) {
    let _ = writeln!(text, "PROPOSALS ({} open)", store.open_count());
    let _ = writeln!(text);
    if store.is_empty() {
        let _ = writeln!(text, "  (none - F9 publishes a sample)");
    }
    for (row, (id, entry)) in store.iter().enumerate() {
        let marker = if row == cursor { ">" } else { " " };
        let (yes, no) = entry.tally();
        let _ = writeln!(
            text,
            "{marker} {}. [{}] {} {:?} by {}  yes {yes} / no {no}",
            row + 1,
            entry.status.tag(),
            id.short(),
            entry.signed.proposal.kind,
            entry.signed.proposal.author_public_key.short(),
        );
    }
    let _ = writeln!(text);
    let _ = writeln!(text, "Up/Down select  Enter details  P close");
}

fn write_detail(
    text: &mut String,
    entry: &ProposalEntry,
    me: PlayerId,
    clock: &EpochClock,
    ledger: &LedgerStore,
) {
    let proposal: &Proposal = &entry.signed.proposal;
    let id = entry.signed.proposal_id();
    let git: String = proposal
        .git_commit_hash
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let _ = writeln!(text, "PROPOSAL {id}");
    let _ = writeln!(text);
    let _ = writeln!(text, "kind       {:?}", proposal.kind);
    let _ = writeln!(text, "author     {}", proposal.author_public_key.short());
    let _ = writeln!(text, "git commit {git}");
    let _ = writeln!(text, "source     {}", proposal.source_bundle_cid.short());
    let _ = writeln!(text, "build      {}", proposal.build_manifest_cid.short());
    let _ = writeln!(text, "tests      {}", proposal.test_results_cid.short());
    let _ = writeln!(
        text,
        "contents   {} wasm, {} asset(s), {} migration(s)",
        proposal.wasm_module_cids.len(),
        proposal.asset_cids.len(),
        proposal.migration_cids.len(),
    );
    if let Some(change) = &proposal.governance_change {
        let _ = writeln!(text, "gov rule   {}", change.rule_module_cid.short());
    }

    // Window status: a live countdown while open, else the closed status.
    let now_epoch = clock.now_epoch();
    if entry.status == ProposalStatus::Open {
        let close_unix = proposal.activation_epoch.saturating_mul(clock.epoch_secs);
        let remaining = close_unix.saturating_sub(clock.now_unix());
        let _ = writeln!(
            text,
            "voting closes in {remaining}s (epoch {}, now {now_epoch})",
            proposal.activation_epoch
        );
    } else {
        let _ = writeln!(text, "status: {}", entry.status.tag());
    }

    let rollback = match &proposal.rollback_plan {
        RollbackPlan::RevertToLastSignedSnapshot => "revert to last signed snapshot".to_owned(),
        RollbackPlan::ReverseMigrations {
            reverse_migration_cids,
        } => format!("{} reverse migration(s)", reverse_migration_cids.len()),
    };
    let _ = writeln!(text, "rollback   {rollback}");
    let _ = writeln!(text);
    let (yes, no) = entry.tally();
    let _ = writeln!(
        text,
        "TALLY yes {yes} / no {no} ({} voter(s))",
        entry.votes.len()
    );

    // For an accepted proposal the certificate is the source of truth; show it.
    if entry.status == ProposalStatus::Accepted
        && let Some(ledger_entry) = ledger.ledger.get(&id)
    {
        let cert = &ledger_entry.certificate;
        let root = cert.certificate.eligible_roster_root();
        let root_hex: String = root[..4].iter().map(|b| format!("{b:02x}")).collect();
        let _ = writeln!(text);
        let _ = writeln!(text, "CERTIFICATE");
        let _ = writeln!(text, "certifier  {}", cert.certifier.short());
        let _ = writeln!(
            text,
            "roster     {} ({root_hex})",
            cert.certificate.eligible_roster.len()
        );
        let _ = writeln!(
            text,
            "votes      yes {} / no {} of {}",
            cert.certificate.yes_votes.len(),
            cert.certificate.no_votes.len(),
            cert.certificate.eligible_roster.len(),
        );
        let _ = writeln!(text, "accepted   epoch {}", cert.certificate.accepted_epoch);
        let _ = writeln!(
            text,
            "rule ver   {}",
            cert.certificate.governance_rule_version
        );
    }

    let mine = match entry.votes.get(&me).map(|sv| sv.vote.choice) {
        Some(VoteChoice::Yes) => "yes",
        Some(VoteChoice::No) => "no",
        None => "none",
    };
    let _ = writeln!(text, "your vote: {mine}");
    let _ = writeln!(text);
    if entry.status == ProposalStatus::Open {
        let _ = writeln!(text, "Y vote yes  N vote no  P back");
    } else {
        let _ = writeln!(text, "voting closed   P back");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use civora_governance::{Cid, ProposalKind, SignedCertificate};
    use civora_identity::Identity;

    /// Window closes at epoch 1000, so `now_epoch < 1000` is "open".
    const OPEN_EPOCH: u64 = 0;
    const CLOSED_EPOCH: u64 = 1000;

    fn identity(seed: u8) -> Identity {
        Identity::from_seed([seed; 32])
    }

    fn proposal(author: &Identity, marker: u8) -> SignedProposal {
        let proposal = Proposal {
            kind: ProposalKind::AssetPatch,
            author_public_key: author.player_id(),
            git_commit_hash: [marker; 20],
            source_bundle_cid: Cid([1; 32]),
            build_manifest_cid: Cid([2; 32]),
            wasm_module_cids: vec![],
            asset_cids: vec![Cid([marker; 32])],
            migration_cids: vec![],
            governance_change: None,
            test_results_cid: Cid([3; 32]),
            activation_epoch: CLOSED_EPOCH,
            rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
        };
        SignedProposal::sign(author, proposal)
    }

    fn ballot(voter: &Identity, id: ProposalId, choice: VoteChoice) -> SignedVote {
        SignedVote::sign(
            voter,
            Vote {
                proposal_id: id,
                voter: voter.player_id(),
                choice,
            },
        )
    }

    #[test]
    fn insert_proposal_is_gated_and_idempotent() {
        let author = identity(1);
        let mut store = ProposalStore::default();
        let signed = proposal(&author, 7);

        assert_eq!(store.insert_proposal(signed.clone()), Ok(true));
        assert_eq!(store.insert_proposal(signed.clone()), Ok(false));
        assert_eq!(store.open_count(), 1);

        // Tampered signature is rejected.
        let mut tampered = signed.clone();
        tampered.signature[0] ^= 1;
        assert_eq!(
            store.insert_proposal(tampered),
            Err(StoreError::BadSignature(VerifyError::BadSignature))
        );

        // A verified but invalid manifest is rejected (asset patch with no
        // content at all).
        let mut empty = signed.proposal.clone();
        empty.asset_cids.clear();
        let empty = SignedProposal::sign(&author, empty);
        assert_eq!(
            store.insert_proposal(empty),
            Err(StoreError::InvalidProposal(ValidationError::EmptyProposal))
        );
        assert_eq!(store.open_count(), 1);
    }

    #[test]
    fn votes_tally_and_revotes_replace() {
        let author = identity(1);
        let voter = identity(2);
        let mut store = ProposalStore::default();
        let signed = proposal(&author, 7);
        let id = signed.proposal_id();
        store.insert_proposal(signed).unwrap();

        store
            .insert_vote(ballot(&voter, id, VoteChoice::Yes), OPEN_EPOCH)
            .unwrap();
        store
            .insert_vote(ballot(&author, id, VoteChoice::No), OPEN_EPOCH)
            .unwrap();
        assert_eq!(store.get(&id).unwrap().tally(), (1, 1));

        // A revote replaces, not adds.
        store
            .insert_vote(ballot(&voter, id, VoteChoice::No), OPEN_EPOCH)
            .unwrap();
        assert_eq!(store.get(&id).unwrap().tally(), (0, 2));

        // A tampered ballot dies at the gate.
        let mut bad = ballot(&voter, id, VoteChoice::Yes);
        bad.vote.choice = VoteChoice::No;
        assert_eq!(
            store.insert_vote(bad, OPEN_EPOCH),
            Err(StoreError::BadSignature(VerifyError::BadSignature))
        );
        assert_eq!(store.get(&id).unwrap().tally(), (0, 2));
    }

    #[test]
    fn early_votes_attach_when_the_proposal_arrives() {
        let author = identity(1);
        let voter = identity(2);
        let mut store = ProposalStore::default();
        let signed = proposal(&author, 7);
        let id = signed.proposal_id();

        // Vote first (gossip raced the proposal); revote while pending.
        store
            .insert_vote(ballot(&voter, id, VoteChoice::No), OPEN_EPOCH)
            .unwrap();
        store
            .insert_vote(ballot(&voter, id, VoteChoice::Yes), OPEN_EPOCH)
            .unwrap();
        assert_eq!(store.pending_votes.len(), 1, "pending revote replaced");

        store.insert_proposal(signed).unwrap();
        assert_eq!(store.get(&id).unwrap().tally(), (1, 0));
        assert!(store.pending_votes.is_empty());
    }

    #[test]
    fn votes_after_close_are_rejected() {
        let author = identity(1);
        let voter = identity(2);
        let mut store = ProposalStore::default();
        let signed = proposal(&author, 7);
        let id = signed.proposal_id();
        store.insert_proposal(signed).unwrap();

        // Past the window (now_epoch >= activation_epoch).
        assert_eq!(
            store.insert_vote(ballot(&voter, id, VoteChoice::Yes), CLOSED_EPOCH),
            Err(StoreError::VotingClosed)
        );
        // Still open, but the proposal is no longer Open (e.g. rejected at
        // close): also refused.
        store.set_status(&id, ProposalStatus::Rejected);
        assert_eq!(
            store.insert_vote(ballot(&voter, id, VoteChoice::Yes), OPEN_EPOCH),
            Err(StoreError::VotingClosed)
        );
        assert_eq!(store.get(&id).unwrap().tally(), (0, 0));
    }

    #[test]
    fn status_drives_open_count_and_closable() {
        let author = identity(1);
        let mut store = ProposalStore::default();
        let signed = proposal(&author, 7);
        let id = signed.proposal_id();
        store.insert_proposal(signed).unwrap();

        assert_eq!(store.open_count(), 1);
        assert_eq!(store.len(), 1);
        assert_eq!(store.closable_proposals(OPEN_EPOCH), vec![]);
        // The window has closed but nobody has certified yet.
        assert_eq!(store.closable_proposals(CLOSED_EPOCH), vec![id]);

        // Once closed, it drops out of the open count but stays visible in the
        // list — `len` (the cursor bound) still counts it.
        store.set_status(&id, ProposalStatus::Expired);
        assert_eq!(store.open_count(), 0);
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
        assert_eq!(store.closable_proposals(CLOSED_EPOCH), vec![]);
        assert!(store.get(&id).is_some());
    }

    #[test]
    fn certificate_parks_until_its_proposal_arrives() {
        let author = identity(1);
        let mut store = ProposalStore::default();
        let signed = proposal(&author, 7);
        let id = signed.proposal_id();

        // Certify solo (roster = author, one yes vote), window closed.
        let ballots: BTreeMap<PlayerId, SignedVote> =
            [(author.player_id(), ballot(&author, id, VoteChoice::Yes))].into();
        let certificate = SignedCertificate::certify(
            &author,
            &signed.proposal,
            &[author.player_id()],
            &ballots,
            1,
            CLOSED_EPOCH,
        )
        .unwrap();

        // The certificate arrives first and is parked.
        store.park_certificate(certificate.clone());
        assert!(store.take_pending_certificate(&id).is_some());
        // Taken once, it is gone.
        store.park_certificate(certificate);
        store.insert_proposal(signed).unwrap();
        assert!(store.take_pending_certificate(&id).is_some());
        assert!(store.take_pending_certificate(&id).is_none());
    }
}
