//! Bevy side of the P2P session: pumps events from the network thread into
//! the sim, answers snapshot requests, publishes beacons, and detects
//! divergence.
//!
//! The Bevy world stays the single owner of [`SimWorld`] and [`SessionLog`];
//! the network thread only ever sees copies (snapshots) or verified inputs
//! (remote actions through the kernel gate in [`apply_remote_actions`]).

use std::sync::Mutex;

use bevy::prelude::*;
use civora_identity::{PlayerId, SignedAction};
use civora_net::wire::{
    MAX_SYNC_LEDGER_ENTRIES, MAX_SYNC_OPEN_PROPOSALS, MAX_SYNC_VOTES, StateBeacon, snapshot_chunks,
};
use civora_net::{NetCommand, NetEvent, NetHandle, Snapshot};
use civora_sim::{ChunkPos, tick};

use crate::identity::{LocalIdentity, SessionLog};
use crate::ledger::{EpochClock, LedgerStore, apply_certificate};
use crate::packs::{ContentStore, PackTracker, track_pack};
use crate::player::Player;
use crate::sim_bridge::{DirtyChunks, SimWorld, drain_action_queue};
use crate::voting::{ProposalStatus, ProposalStore};

/// One beacon per 100 fixed ticks = every 5 s at 20 Hz.
const BEACON_INTERVAL_TICKS: u32 = 100;

/// Where a fresh join drops the player (matches the initial spawn).
const SPAWN_POS: Vec3 = Vec3::new(0.5, 8.0, 0.5);

pub struct NetPlugin {
    /// Taken (once) by `build`; `Plugin::build` only gets `&self`.
    pub handle: Mutex<Option<NetHandle>>,
    pub joining: bool,
}

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PeerRoster>()
            .init_resource::<RemoteActionQueue>()
            // Systems are always registered but inert until a session
            // installs NetChannels — at boot (CLI flags) or from the menu.
            .add_systems(
                FixedUpdate,
                (
                    (pump_net_events, apply_remote_actions)
                        .chain()
                        .before(drain_action_queue),
                    publish_beacon.after(drain_action_queue),
                )
                    .run_if(resource_exists::<NetChannels>),
            );

        match self.handle.lock().unwrap().take() {
            Some(handle) => {
                app.insert_resource(NetStatus {
                    phase: if self.joining {
                        NetPhase::Joining
                    } else {
                        NetPhase::Host
                    },
                    ..NetStatus::offline()
                })
                .insert_resource(NetChannels::new(handle));
            }
            None => {
                app.insert_resource(NetStatus::offline());
            }
        }
    }
}

/// Start a session chosen from the start screen: spawn the network thread
/// and install its channels so the (already registered) net systems engage.
pub fn start_session(
    commands: &mut Commands,
    status: &mut NetStatus,
    seed: [u8; 32],
    mode: civora_net::SessionMode,
) {
    let joining = matches!(mode, civora_net::SessionMode::Join { .. });
    let handle = civora_net::spawn(civora_net::NetConfig {
        seed,
        mode,
        cell: civora_net::wire::CellRef::genesis(),
        enable_mdns: true,
    });
    commands.insert_resource(NetChannels::new(handle));
    status.phase = if joining {
        NetPhase::Joining
    } else {
        NetPhase::Host
    };
}

/// Channels to the network thread. The event receiver is `Send + !Sync`,
/// hence the mutex; only [`pump_net_events`] locks it.
#[derive(Resource)]
pub struct NetChannels {
    pub commands: civora_net::CommandSender,
    events: Mutex<std::sync::mpsc::Receiver<NetEvent>>,
}

impl NetChannels {
    pub fn new(handle: NetHandle) -> Self {
        Self {
            commands: handle.commands,
            events: Mutex::new(handle.events),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum NetPhase {
    Offline,
    Host,
    /// Waiting for the world snapshot; input and the sim gate are held.
    Joining,
    Live,
}

#[derive(Resource)]
pub struct NetStatus {
    pub phase: NetPhase,
    pub listen_addr: Option<String>,
    pub last_error: Option<String>,
    pub diverged: bool,
    /// Per-author seq deficit seen in the previous beacon. Resync fires only
    /// when the same deficit persists across two beacons, so gossip still in
    /// flight doesn't trigger spurious resyncs.
    prev_deficit: Vec<(PlayerId, u64)>,
}

impl NetStatus {
    fn offline() -> Self {
        Self {
            phase: NetPhase::Offline,
            listen_addr: None,
            last_error: None,
            diverged: false,
            prev_deficit: Vec::new(),
        }
    }

    /// While joining, input and local actions are held.
    pub fn gate_input(&self) -> bool {
        self.phase == NetPhase::Joining
    }
}

/// Connected peers, by player id and remote address.
#[derive(Resource, Default)]
pub struct PeerRoster(pub Vec<(PlayerId, String)>);

/// Gossiped actions waiting for the kernel gate on the next fixed tick.
#[derive(Resource, Default)]
pub struct RemoteActionQueue(pub Vec<SignedAction>);

#[allow(clippy::too_many_arguments)]
fn pump_net_events(
    channels: Res<NetChannels>,
    mut status: ResMut<NetStatus>,
    mut roster: ResMut<PeerRoster>,
    mut remote: ResMut<RemoteActionQueue>,
    mut world: ResMut<SimWorld>,
    mut log: ResMut<SessionLog>,
    mut local: ResMut<LocalIdentity>,
    mut dirty: ResMut<DirtyChunks>,
    mut store: ResMut<ProposalStore>,
    mut ledger: ResMut<LedgerStore>,
    mut tracker: ResMut<PackTracker>,
    content: Res<ContentStore>,
    clock: Res<EpochClock>,
    mut player: Single<(&mut Transform, &mut Player)>,
) {
    // Drain without holding the lock across the loop body borrows.
    let events: Vec<NetEvent> = channels.events.lock().unwrap().try_iter().collect();
    for event in events {
        match event {
            NetEvent::ListeningOn { addr } => {
                // Loopback listeners aren't useful join targets to print.
                if status.listen_addr.is_none() || !addr.starts_with("/ip4/127.") {
                    info!("listening on {addr}");
                    status.listen_addr = Some(addr);
                }
            }
            NetEvent::PeerConnected { player, addr } => {
                info!("peer {} connected from {addr}", player.short());
                roster.0.retain(|(id, _)| *id != player);
                roster.0.push((player, addr));
            }
            NetEvent::PeerDisconnected { player } => {
                info!("peer {} disconnected", player.short());
                roster.0.retain(|(id, _)| *id != player);
            }
            NetEvent::SnapshotRequested { request_id } => {
                let snapshot = build_snapshot(&world, &log, &ledger, &store);
                let _ = channels.commands.send(NetCommand::ProvideSnapshot {
                    request_id,
                    snapshot,
                });
            }
            NetEvent::WorldSync {
                world: synced,
                log: synced_log,
                content_hash,
            } => {
                let old: Vec<ChunkPos> = world.0.chunk_positions().collect();
                world.0 = synced;
                dirty.0.extend(old);
                dirty.0.extend(world.0.chunk_positions());
                log.0 = synced_log;
                // Resume our own numbering where the transferred log left it
                // (a rejoin with the same identity must not replay seqs).
                local.next_seq = log
                    .0
                    .last_seq(local.identity.player_id())
                    .map_or(0, |seq| seq + 1);
                let (transform, player) = &mut *player;
                transform.translation = SPAWN_POS;
                player.velocity = Vec3::ZERO;
                status.phase = NetPhase::Live;
                status.diverged = false;
                status.prev_deficit.clear();
                status.last_error = None;
                info!(
                    "world synced: {} signed actions, hash {content_hash:016x}",
                    log.0.len()
                );
            }
            NetEvent::RemoteAction(signed) => remote.0.push(signed),
            NetEvent::RemoteProposal(signed) => {
                let id = signed.proposal_id();
                match store.insert_proposal(*signed) {
                    Ok(true) => {
                        info!("proposal {} received", id.short());
                        // A certificate may have arrived before its proposal.
                        if let Some(cert) = store.take_pending_certificate(&id)
                            && let Some(proposal) = apply_certificate(&mut store, &mut ledger, cert)
                        {
                            track_pack(&mut tracker, &content.0, Some(&*channels), id, &proposal);
                        }
                    }
                    // Gossip redelivery is normal; forged manifests die here.
                    Ok(false) => debug!("proposal {} already known", id.short()),
                    Err(err) => debug!("dropped remote proposal: {err}"),
                }
            }
            NetEvent::RemoteVote(signed) => {
                if let Err(err) = store.insert_vote(signed, clock.now_epoch()) {
                    debug!("dropped remote vote: {err}");
                }
            }
            NetEvent::RemoteCertificate(signed) => {
                if let Some(proposal) = apply_certificate(&mut store, &mut ledger, *signed) {
                    track_pack(
                        &mut tracker,
                        &content.0,
                        Some(&*channels),
                        proposal.id(),
                        &proposal,
                    );
                }
            }
            NetEvent::BlobRequested { request_id, cid } => {
                // Serve from our store; a corrupt/unreadable blob answers
                // not-found rather than poisoning the requester.
                let bytes = match content.0.get(&cid) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        warn!("serving blob {}: {err}", cid.short());
                        None
                    }
                };
                let _ = channels
                    .commands
                    .send(NetCommand::ProvideBlob { request_id, bytes });
            }
            NetEvent::BlobFetched { cid, bytes } => {
                // Bytes already hash to cid (checked in the net layer); store and
                // clear it from every pack that wanted it.
                match content.0.put(&bytes) {
                    Ok(_) => {
                        tracker.on_fetched(&cid);
                        info!("fetched blob {} ({} bytes)", cid.short(), bytes.len());
                    }
                    Err(err) => warn!("storing fetched blob {}: {err}", cid.short()),
                }
            }
            NetEvent::BlobFetchFailed { cid, reason } => {
                tracker.on_failed(&cid, &reason);
                debug!("blob {} fetch failed: {reason}", cid.short());
            }
            NetEvent::RemoteBeacon { from, beacon } => {
                check_beacon(&channels, &mut status, &world, &log, &local, from, &beacon);
            }
            NetEvent::JoinFailed { reason } => {
                warn!("join failed: {reason}");
                status.last_error = Some(reason);
            }
            NetEvent::Fatal { reason } => {
                error!("network thread died: {reason}");
                status.last_error = Some(reason);
                status.phase = NetPhase::Offline;
                roster.0.clear();
                // In-flight fetches can never complete now; the retry timer
                // re-requests once a session returns.
                tracker.clear_in_flight();
            }
        }
    }
}

/// Assemble the join-response snapshot: the world and log, plus the governance
/// state a late joiner needs. Each governance list is truncated to its wire
/// cap (a warn if it overflows; alpha ledgers are far smaller — announce-then
/// fetch is the patch-pack milestone).
fn build_snapshot(
    world: &SimWorld,
    log: &SessionLog,
    ledger: &LedgerStore,
    store: &ProposalStore,
) -> Snapshot {
    let entries = ledger.ledger.entries();
    if entries.len() > MAX_SYNC_LEDGER_ENTRIES {
        warn!(
            "ledger has {} entries; truncating to {MAX_SYNC_LEDGER_ENTRIES} for join sync",
            entries.len()
        );
    }
    let ledger_payload = entries
        .iter()
        .take(MAX_SYNC_LEDGER_ENTRIES)
        .map(|e| (e.proposal.clone(), e.certificate.clone()))
        .collect();

    let mut open_proposals = Vec::new();
    let mut open_votes = Vec::new();
    for (_, entry) in store.iter() {
        if entry.status != ProposalStatus::Open {
            continue;
        }
        if open_proposals.len() < MAX_SYNC_OPEN_PROPOSALS {
            open_proposals.push(entry.signed.clone());
            for signed in entry.votes.values() {
                if open_votes.len() < MAX_SYNC_VOTES {
                    open_votes.push(*signed);
                }
            }
        }
    }

    Snapshot {
        content_hash: world.0.content_hash(),
        log: log.0.entries().to_vec(),
        chunks: snapshot_chunks(&world.0),
        ledger: ledger_payload,
        open_proposals,
        open_votes,
    }
}

/// Compare a peer's beacon against our state; request a resync when we
/// verifiably missed gossip or truly diverged.
fn check_beacon(
    channels: &NetChannels,
    status: &mut NetStatus,
    world: &SimWorld,
    log: &SessionLog,
    local: &LocalIdentity,
    from: PlayerId,
    beacon: &StateBeacon,
) {
    if status.phase != NetPhase::Live && status.phase != NetPhase::Host {
        return;
    }

    // Authors whose actions the peer has accepted but we haven't.
    let deficit: Vec<(PlayerId, u64)> = beacon
        .seqs
        .iter()
        .filter(|(author, seq)| log.0.last_seq(*author).is_none_or(|mine| mine < *seq))
        .copied()
        .collect();
    if !deficit.is_empty() {
        if status.prev_deficit == deficit {
            warn!(
                "missed gossip (behind {} author(s) per {}), resyncing",
                deficit.len(),
                from.short()
            );
            status.diverged = true;
            let _ = channels.commands.send(NetCommand::Resync {
                preferred: Some(from),
            });
            status.prev_deficit.clear();
        } else {
            status.prev_deficit = deficit;
        }
        return;
    }
    status.prev_deficit.clear();

    // Same set of accepted actions but a different world: application order
    // diverged. Deterministic tiebreak: the larger player id yields.
    if beacon.seqs == log.0.seq_vector() && beacon.content_hash != world.0.content_hash() {
        status.diverged = true;
        if local.identity.player_id().0 > from.0 {
            warn!("world diverged from {}, resyncing (we yield)", from.short());
            let _ = channels.commands.send(NetCommand::Resync {
                preferred: Some(from),
            });
        } else {
            warn!(
                "world diverged from {}, waiting for them to resync",
                from.short()
            );
        }
    } else if status.diverged {
        status.diverged = false; // healed
    }
}

/// The kernel gate for gossiped actions: verify into the session log
/// (signature + per-author seq), then — and only then — apply to the world.
fn apply_remote_actions(
    mut remote: ResMut<RemoteActionQueue>,
    mut world: ResMut<SimWorld>,
    mut log: ResMut<SessionLog>,
    mut dirty: ResMut<DirtyChunks>,
    status: Res<NetStatus>,
) {
    if remote.0.is_empty() || status.phase == NetPhase::Joining {
        return;
    }
    let mut verified = Vec::new();
    for signed in remote.0.drain(..) {
        match log.0.append(signed) {
            Ok(()) => verified.push(signed.action),
            // Redelivery is normal for gossip; anything already in the log
            // fails the seq check here and is dropped silently.
            Err(err) => debug!("dropped remote action: {err}"),
        }
    }
    for chunk_pos in tick::step(&mut world.0, &verified) {
        dirty.0.insert(chunk_pos);
    }
}

fn publish_beacon(
    channels: Res<NetChannels>,
    status: Res<NetStatus>,
    world: Res<SimWorld>,
    log: Res<SessionLog>,
    roster: Res<PeerRoster>,
    mut ticks: Local<u32>,
) {
    if status.phase == NetPhase::Joining || roster.0.is_empty() {
        return;
    }
    *ticks += 1;
    if *ticks < BEACON_INTERVAL_TICKS {
        return;
    }
    *ticks = 0;
    let _ = channels
        .commands
        .send(NetCommand::PublishBeacon(StateBeacon {
            log_len: log.0.len() as u64,
            seqs: log.0.seq_vector(),
            content_hash: world.0.content_hash(),
        }));
}
