//! Dev tooling: screenshots via F12, or automatically after a delay when
//! `CIVORA_SCREENSHOT=<path>` is set (used for scripted verification).
//! F9 publishes a sample proposal so the voting UI is demoable before the
//! real commit-to-proposal pipeline exists; `CIVORA_TEST_PROPOSAL` does the
//! same automatically for scripted runs. A sample proposal puts real content
//! into the local blob store and references it, so patch-pack fetch is
//! exercised end to end; `CIVORA_TEST_VOTE` auto-votes yes on open proposals.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use civora_governance::{
    BlobStore, Cid, Proposal, ProposalId, ProposalKind, RollbackPlan, SignedProposal, SignedVote,
    Vote, VoteChoice,
};
use civora_identity::Identity;

use crate::identity::LocalIdentity;
use crate::ledger::EpochClock;
use crate::net::NetChannels;
use crate::packs::ContentStore;
use crate::voting::{ProposalStatus, ProposalStore};

/// With `CIVORA_TEST_VOTE=1`, auto-vote yes on every open proposal (the joiner
/// side of a scripted two-instance run).
const TEST_VOTE_ENV: &str = "CIVORA_TEST_VOTE";

/// Epochs from now until a sample proposal's window closes: at the default
/// 30 s epoch that is 60-90 s; with `CIVORA_EPOCH_SECS=2` it is a handful of
/// seconds, fast enough for a scripted screenshot.
const DEMO_ACTIVATION_EPOCHS: u64 = 3;

pub struct DebugPlugin;

impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (screenshot_on_f12, auto_screenshot))
            .add_systems(
                Update,
                (
                    publish_sample_proposal_on_f9,
                    auto_publish_sample_proposal,
                    auto_vote_yes_on_open,
                )
                    .run_if(in_state(crate::AppState::InGame)),
            );
    }
}

/// A distinct, valid proposal manifest per press, with **real content**: five
/// deterministic blobs (source/build/tests text plus two binary assets) are put
/// into the local store and the manifest references their cids, so an accepted
/// sample resolves its pack over the fetch protocol just like a real patch. The
/// `git_commit_hash` stays label-derived — it is provenance, not fetched. The
/// window closes [`DEMO_ACTIVATION_EPOCHS`] epochs from `now_epoch`.
fn sample_proposal(author: &Identity, n: u32, now_epoch: u64, store: &BlobStore) -> Proposal {
    let put_text = |label: &str| {
        store
            .put(format!("civora sample {label} {n}\n").as_bytes())
            .expect("blob store put")
    };
    // Two distinct multi-KiB binary assets derived from n.
    let asset = |variant: u8| -> Vec<u8> {
        (0..4096u32)
            .map(|i| (i as u8) ^ (n as u8) ^ variant)
            .collect()
    };
    let mut asset_cids = vec![
        store.put(&asset(0)).expect("blob store put"),
        store.put(&asset(1)).expect("blob store put"),
    ];
    asset_cids.sort();
    asset_cids.dedup();

    let mut git_commit_hash = [0u8; 20];
    git_commit_hash
        .copy_from_slice(&Cid::of(format!("civora sample commit {n}").as_bytes()).0[..20]);
    let proposal = Proposal {
        kind: ProposalKind::AssetPatch,
        author_public_key: author.player_id(),
        git_commit_hash,
        source_bundle_cid: put_text("source"),
        build_manifest_cid: put_text("build"),
        wasm_module_cids: Vec::new(),
        asset_cids,
        migration_cids: Vec::new(),
        governance_change: None,
        test_results_cid: put_text("tests"),
        activation_epoch: now_epoch + DEMO_ACTIVATION_EPOCHS,
        rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
    };
    proposal.validate().expect("sample proposal is valid");
    proposal
}

/// Sign a sample proposal, insert it through the same gate remote ones pass,
/// and gossip it (gossipsub does not loop back to the publisher). When
/// `auto_vote_yes`, also cast and gossip a yes ballot so a scripted solo run
/// reaches acceptance unattended.
#[allow(clippy::too_many_arguments)]
fn publish_sample(
    counter: &mut u32,
    local: &LocalIdentity,
    store: &mut ProposalStore,
    content: &BlobStore,
    channels: Option<&NetChannels>,
    now_epoch: u64,
    auto_vote_yes: bool,
) {
    let n = *counter;
    *counter += 1;
    let signed = SignedProposal::sign(
        &local.identity,
        sample_proposal(&local.identity, n, now_epoch, content),
    );
    let id = signed.proposal_id();
    store
        .insert_proposal(signed.clone())
        .expect("locally signed sample proposal verifies");
    if let Some(channels) = channels {
        let _ = channels
            .commands
            .send(civora_net::NetCommand::PublishProposal(Box::new(signed)));
    }
    info!("published sample proposal {}", id.short());

    if auto_vote_yes {
        let ballot = SignedVote::sign(
            &local.identity,
            Vote {
                proposal_id: id,
                voter: local.identity.player_id(),
                choice: VoteChoice::Yes,
            },
        );
        store
            .insert_vote(ballot, now_epoch)
            .expect("locally signed ballot verifies within the window");
        if let Some(channels) = channels {
            let _ = channels
                .commands
                .send(civora_net::NetCommand::PublishVote(ballot));
        }
    }
}

fn publish_sample_proposal_on_f9(
    keys: Res<ButtonInput<KeyCode>>,
    local: Res<LocalIdentity>,
    clock: Res<EpochClock>,
    mut store: ResMut<ProposalStore>,
    content: Res<ContentStore>,
    channels: Option<Res<NetChannels>>,
    mut counter: Local<u32>,
) {
    if keys.just_pressed(KeyCode::F9) {
        publish_sample(
            &mut counter,
            &local,
            &mut store,
            &content.0,
            channels.as_deref(),
            clock.now_epoch(),
            false,
        );
    }
}

/// With `CIVORA_TEST_PROPOSAL` set, publish one sample shortly after startup
/// and auto-vote yes on it — pairs with `CIVORA_SCREENSHOT` for scripted UI
/// verification of the accepted path.
#[allow(clippy::too_many_arguments)]
fn auto_publish_sample_proposal(
    local: Res<LocalIdentity>,
    clock: Res<EpochClock>,
    mut store: ResMut<ProposalStore>,
    content: Res<ContentStore>,
    channels: Option<Res<NetChannels>>,
    time: Res<Time>,
    mut counter: Local<u32>,
    mut done: Local<bool>,
) {
    if *done || time.elapsed_secs() < 3.0 {
        return;
    }
    *done = true;
    if std::env::var("CIVORA_TEST_PROPOSAL").is_ok() {
        publish_sample(
            &mut counter,
            &local,
            &mut store,
            &content.0,
            channels.as_deref(),
            clock.now_epoch(),
            true,
        );
    }
}

/// With `CIVORA_TEST_VOTE` set, cast (and gossip) a yes ballot on every open
/// proposal we have not yet voted on — the joiner side of a scripted run, where
/// the host publishes and this instance approves.
fn auto_vote_yes_on_open(
    local: Res<LocalIdentity>,
    clock: Res<EpochClock>,
    mut store: ResMut<ProposalStore>,
    channels: Option<Res<NetChannels>>,
) {
    if std::env::var(TEST_VOTE_ENV).is_err() {
        return;
    }
    let me = local.identity.player_id();
    let now_epoch = clock.now_epoch();
    let unvoted: Vec<ProposalId> = store
        .iter()
        .filter(|(_, entry)| entry.status == ProposalStatus::Open && !entry.votes.contains_key(&me))
        .map(|(id, _)| *id)
        .collect();
    for id in unvoted {
        let ballot = SignedVote::sign(
            &local.identity,
            Vote {
                proposal_id: id,
                voter: me,
                choice: VoteChoice::Yes,
            },
        );
        if store.insert_vote(ballot, now_epoch).is_ok() {
            if let Some(channels) = &channels {
                let _ = channels
                    .commands
                    .send(civora_net::NetCommand::PublishVote(ballot));
            }
            info!("auto-voted yes on {}", id.short());
        }
    }
}

fn screenshot_on_f12(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut counter: Local<u32>,
) {
    if keys.just_pressed(KeyCode::F12) {
        let path = format!("./civora-screenshot-{}.png", *counter);
        *counter += 1;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
}

fn auto_screenshot(mut commands: Commands, time: Res<Time>, mut done: Local<bool>) {
    let delay: f32 = std::env::var("CIVORA_SCREENSHOT_DELAY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2.0);
    if *done || time.elapsed_secs() < delay {
        return;
    }
    *done = true;
    if let Ok(path) = std::env::var("CIVORA_SCREENSHOT") {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
}
