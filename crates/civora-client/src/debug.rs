//! Dev tooling: screenshots via F12, or automatically after a delay when
//! `CIVORA_SCREENSHOT=<path>` is set (used for scripted verification).
//! F9 publishes a sample proposal so the voting UI is demoable before the
//! real commit-to-proposal pipeline exists; `CIVORA_TEST_PROPOSAL` does the
//! same automatically for scripted runs.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use civora_governance::{
    Cid, Proposal, ProposalKind, RollbackPlan, SignedProposal, SignedVote, Vote, VoteChoice,
};
use civora_identity::Identity;

use crate::identity::LocalIdentity;
use crate::ledger::EpochClock;
use crate::net::NetChannels;
use crate::voting::ProposalStore;

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
                (publish_sample_proposal_on_f9, auto_publish_sample_proposal)
                    .run_if(in_state(crate::AppState::InGame)),
            );
    }
}

/// A distinct, valid proposal manifest per press. The cids are hashes of
/// labels rather than real artifacts — there is no patch pipeline yet. The
/// window closes [`DEMO_ACTIVATION_EPOCHS`] epochs from `now_epoch`.
fn sample_proposal(author: &Identity, n: u32, now_epoch: u64) -> Proposal {
    let labeled = |label: &str| Cid::of(format!("civora sample {label} {n}").as_bytes());
    let mut git_commit_hash = [0u8; 20];
    git_commit_hash.copy_from_slice(&labeled("commit").0[..20]);
    let proposal = Proposal {
        kind: ProposalKind::AssetPatch,
        author_public_key: author.player_id(),
        git_commit_hash,
        source_bundle_cid: labeled("source"),
        build_manifest_cid: labeled("build"),
        wasm_module_cids: Vec::new(),
        asset_cids: vec![Cid::of(&n.to_le_bytes())],
        migration_cids: Vec::new(),
        governance_change: None,
        test_results_cid: labeled("tests"),
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
fn publish_sample(
    counter: &mut u32,
    local: &LocalIdentity,
    store: &mut ProposalStore,
    channels: Option<&NetChannels>,
    now_epoch: u64,
    auto_vote_yes: bool,
) {
    let n = *counter;
    *counter += 1;
    let signed = SignedProposal::sign(
        &local.identity,
        sample_proposal(&local.identity, n, now_epoch),
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
    channels: Option<Res<NetChannels>>,
    mut counter: Local<u32>,
) {
    if keys.just_pressed(KeyCode::F9) {
        publish_sample(
            &mut counter,
            &local,
            &mut store,
            channels.as_deref(),
            clock.now_epoch(),
            false,
        );
    }
}

/// With `CIVORA_TEST_PROPOSAL` set, publish one sample shortly after startup
/// and auto-vote yes on it — pairs with `CIVORA_SCREENSHOT` for scripted UI
/// verification of the accepted path.
fn auto_publish_sample_proposal(
    local: Res<LocalIdentity>,
    clock: Res<EpochClock>,
    mut store: ResMut<ProposalStore>,
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
            channels.as_deref(),
            clock.now_epoch(),
            true,
        );
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
