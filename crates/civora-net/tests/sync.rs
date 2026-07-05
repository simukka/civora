//! Two-node lobby + world sync integration test.
//!
//! Runs two real network threads (real TCP on 127.0.0.1, real Noise, real
//! gossipsub) with the test playing each node's "client" role: answering
//! snapshot requests, appending remote actions through the kernel gate, and
//! applying them with `tick::step`. mDNS stays off so the test is
//! deterministic and CI-safe; the joiner direct-dials the host.

use std::sync::mpsc::Receiver;
use std::time::Duration;

use civora_identity::{ActionLog, Identity, VerifyError};
use civora_net::wire::{CellRef, snapshot_chunks};
use civora_net::{NetCommand, NetConfig, NetEvent, NetHandle, SessionMode, Snapshot};
use civora_sim::{Action, BlockId, VoxelWorld, tick};

const EVENT_TIMEOUT: Duration = Duration::from_secs(20);

/// Wait for the first event `pick` accepts, skipping others (peer/listen
/// noise arrives in nondeterministic order).
fn wait_for<T>(
    events: &Receiver<NetEvent>,
    what: &str,
    mut pick: impl FnMut(NetEvent) -> Option<T>,
) -> T {
    let deadline = std::time::Instant::now() + EVENT_TIMEOUT;
    loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .unwrap_or_else(|| panic!("timed out waiting for {what}"));
        match events.recv_timeout(remaining) {
            Ok(event) => {
                if let Some(found) = pick(event) {
                    return found;
                }
            }
            Err(err) => panic!("waiting for {what}: {err}"),
        }
    }
}

fn place(pos: [i32; 3], block: BlockId) -> Action {
    Action::PlaceBlock { pos, block }
}

/// A network node plus the sim state its "client" would own.
struct TestNode {
    net: NetHandle,
    identity: Identity,
    world: VoxelWorld,
    log: ActionLog,
    next_seq: u64,
}

impl TestNode {
    fn start(seed: u8, mode: SessionMode) -> Self {
        let identity = Identity::from_seed([seed; 32]);
        let net = civora_net::spawn(NetConfig {
            seed: identity.seed_bytes(),
            mode,
            cell: CellRef::genesis(),
            enable_mdns: false,
        });
        Self {
            net,
            identity,
            world: VoxelWorld::new(),
            log: ActionLog::new(),
            next_seq: 0,
        }
    }

    /// The kernel-gate path a client runs per local action: sign, append,
    /// apply, then gossip.
    fn act(&mut self, action: Action) {
        let signed = self.identity.sign(action, self.next_seq);
        self.log.append(signed).expect("local action verifies");
        self.next_seq += 1;
        tick::step(&mut self.world, &[action]);
        self.net
            .commands
            .send(NetCommand::PublishAction(signed))
            .unwrap();
    }

    /// The client's remote-action path: verify into the log, then apply.
    fn apply_remote(&mut self, signed: civora_identity::SignedAction) -> Result<(), VerifyError> {
        self.log.append(signed)?;
        tick::step(&mut self.world, &[signed.action]);
        Ok(())
    }

    fn serve_snapshot(&self, request_id: u64) {
        self.net
            .commands
            .send(NetCommand::ProvideSnapshot {
                request_id,
                snapshot: Snapshot {
                    content_hash: self.world.content_hash(),
                    log: self.log.entries().to_vec(),
                    chunks: snapshot_chunks(&self.world),
                },
            })
            .unwrap();
    }
}

#[test]
fn two_nodes_join_gossip_and_resync() {
    // --- Host starts with history: a flat world plus two signed edits.
    let mut host = TestNode::start(1, SessionMode::Host);
    host.world = VoxelWorld::flat(1);
    host.act(place([1, 4, 1], BlockId::PLANK));
    host.act(Action::BreakBlock { pos: [2, 3, 2] });
    let host_addr = wait_for(&host.net.events, "host listen addr", |event| match event {
        NetEvent::ListeningOn { addr } if addr.contains("127.0.0.1") => Some(addr),
        _ => None,
    });

    // --- Joiner dials, requests a snapshot, and must reproduce the hash.
    let mut joiner = TestNode::start(
        2,
        SessionMode::Join {
            dial: Some(host_addr),
        },
    );

    // Events arrive in order, so consume each side's PeerConnected before
    // the sync events that follow it (wait_for discards skipped events).
    wait_for(&host.net.events, "host sees joiner", |event| {
        matches!(&event, NetEvent::PeerConnected { player, .. }
            if *player == joiner.identity.player_id())
        .then_some(())
    });
    let request_id = wait_for(&host.net.events, "snapshot request", |event| match event {
        NetEvent::SnapshotRequested { request_id } => Some(request_id),
        _ => None,
    });
    host.serve_snapshot(request_id);

    wait_for(&joiner.net.events, "joiner sees host", |event| {
        matches!(&event, NetEvent::PeerConnected { player, .. }
            if *player == host.identity.player_id())
        .then_some(())
    });
    let (world, log, content_hash) =
        wait_for(&joiner.net.events, "world sync", |event| match event {
            NetEvent::WorldSync {
                world,
                log,
                content_hash,
            } => Some((world, log, content_hash)),
            NetEvent::JoinFailed { reason } => panic!("join failed: {reason}"),
            _ => None,
        });
    assert_eq!(content_hash, host.world.content_hash());
    assert_eq!(world.content_hash(), host.world.content_hash());
    assert_eq!(log.len(), 2, "transferred log arrived intact");
    joiner.world = world;
    joiner.log = log;
    // Same-identity rejoin would resume from the transferred log; identity
    // 2 has no history yet.
    assert_eq!(joiner.log.last_seq(joiner.identity.player_id()), None);

    // --- Live gossip: give the gossipsub subscription exchange a moment,
    // then edit on each side and verify convergence on the other.
    std::thread::sleep(Duration::from_millis(2000));

    joiner.act(place([3, 4, 3], BlockId::GLASS));
    let from_joiner = wait_for(&host.net.events, "gossiped action", |event| match event {
        NetEvent::RemoteAction(signed) => Some(signed),
        _ => None,
    });
    assert_eq!(from_joiner.author, joiner.identity.player_id());
    host.apply_remote(from_joiner).expect("verifies at host");

    host.act(place([4, 4, 4], BlockId::STONE));
    let from_host = wait_for(
        &joiner.net.events,
        "gossiped action back",
        |event| match event {
            NetEvent::RemoteAction(signed) => Some(signed),
            _ => None,
        },
    );
    joiner.apply_remote(from_host).expect("verifies at joiner");
    assert_eq!(
        host.world.content_hash(),
        joiner.world.content_hash(),
        "worlds converged after bidirectional gossip"
    );

    // --- A replayed signed action dies at the kernel gate.
    assert!(matches!(
        joiner.log.append(from_host),
        Err(VerifyError::SeqReplay { .. })
    ));

    // --- Resync: simulate divergence recovery by re-running the join flow.
    host.act(place([5, 4, 5], BlockId::PLANK)); // joiner "misses" this
    let _missed = wait_for(&joiner.net.events, "missed action", |event| match event {
        NetEvent::RemoteAction(signed) => Some(signed),
        _ => None,
    }); // deliberately not applied: the joiner is now behind

    joiner
        .net
        .commands
        .send(NetCommand::Resync {
            preferred: Some(host.identity.player_id()),
        })
        .unwrap();
    let request_id = wait_for(
        &host.net.events,
        "resync snapshot request",
        |event| match event {
            NetEvent::SnapshotRequested { request_id } => Some(request_id),
            _ => None,
        },
    );
    host.serve_snapshot(request_id);
    let (world, log, _) = wait_for(&joiner.net.events, "resync world", |event| match event {
        NetEvent::WorldSync {
            world,
            log,
            content_hash,
        } => Some((world, log, content_hash)),
        NetEvent::JoinFailed { reason } => panic!("resync failed: {reason}"),
        _ => None,
    });
    joiner.world = world;
    joiner.log = log;
    assert_eq!(
        joiner.world.content_hash(),
        host.world.content_hash(),
        "resync healed the divergence"
    );
    // The rebuilt log knows the joiner's own seq history, so numbering
    // resumes without replays.
    assert_eq!(
        joiner.log.last_seq(joiner.identity.player_id()),
        Some(joiner.next_seq - 1)
    );
}
