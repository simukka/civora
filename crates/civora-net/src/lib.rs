//! P2P networking for Civora: lobby (discovery + join) and world cell sync.
//!
//! Implements the "signed action log + periodic snapshots" sync model for
//! voxel edits: a joining peer receives a world snapshot plus the full
//! signed action log, verifies both, then exchanges live [`SignedAction`]s
//! over gossipsub. No authoritative game server; every peer validates every
//! action through the same kernel gate ([`civora_identity::ActionLog`]).
//!
//! This crate knows nothing about Bevy. The client talks to the network
//! thread through the channels in [`NetHandle`]; libp2p types never cross
//! that boundary — peers are identified by [`PlayerId`] on the client side.

mod behaviour;
mod codec;
mod event_loop;
pub mod peer;
pub mod wire;

use civora_identity::{ActionLog, PlayerId, SignedAction};
use civora_sim::{ChunkPos, VoxelWorld};

pub use event_loop::run;
use wire::{CellRef, StateBeacon};

/// Configuration for one networking session.
pub struct NetConfig {
    /// The player identity seed; also becomes the libp2p transport key so
    /// `PeerId == PlayerId`. Secret material.
    pub seed: [u8; 32],
    pub mode: SessionMode,
    pub cell: CellRef,
    /// LAN peer discovery. Disable when only direct dialing is wanted
    /// (integration tests, multicast-hostile networks).
    pub enable_mdns: bool,
}

#[derive(Clone, Debug)]
pub enum SessionMode {
    /// Serve the current world to joiners; also discover/accept peers.
    Host,
    /// Join an existing session: dial `dial` if given, otherwise the first
    /// mDNS-discovered peer, then request a snapshot before going live.
    Join { dial: Option<String> },
}

/// World snapshot the client hands to the net thread to answer a join
/// request (the client owns the world; the net thread never locks it).
pub struct Snapshot {
    pub content_hash: u64,
    pub log: Vec<SignedAction>,
    /// Canonical order: sorted by [`ChunkPos`], empty chunks omitted
    /// (build with [`wire::snapshot_chunks`]).
    pub chunks: Vec<(ChunkPos, Vec<u8>)>,
}

/// Client → network thread.
pub enum NetCommand {
    /// Gossip a locally signed action that passed the kernel gate.
    PublishAction(SignedAction),
    /// Gossip this cell's periodic state beacon.
    PublishBeacon(StateBeacon),
    /// Answer a [`NetEvent::SnapshotRequested`].
    ProvideSnapshot { request_id: u64, snapshot: Snapshot },
    /// Re-run the join flow (divergence recovery), preferably against
    /// `preferred` if it is still connected.
    Resync { preferred: Option<PlayerId> },
}

/// Network thread → client. Ordering is significant: events arrive in the
/// order they happened (e.g. a `SnapshotRequested` after a `WorldSync` is
/// answered from the synced world).
pub enum NetEvent {
    /// We are reachable at `addr` (includes the `/p2p/…` suffix to share).
    ListeningOn {
        addr: String,
    },
    PeerConnected {
        player: PlayerId,
        addr: String,
    },
    PeerDisconnected {
        player: PlayerId,
    },
    /// A joiner asked for the world; reply with
    /// [`NetCommand::ProvideSnapshot`] using the same `request_id`.
    SnapshotRequested {
        request_id: u64,
    },
    /// Join (or resync) succeeded: replace local state with this verified
    /// world and log. `world.content_hash()` already matched
    /// `content_hash`, and every log entry re-verified on append.
    WorldSync {
        world: VoxelWorld,
        log: ActionLog,
        content_hash: u64,
    },
    /// A gossiped action from another peer. Not yet verified — feed it
    /// through [`ActionLog::append`] before applying.
    RemoteAction(SignedAction),
    /// Another peer's state beacon, for divergence detection.
    RemoteBeacon {
        from: PlayerId,
        beacon: StateBeacon,
    },
    /// A join or resync attempt failed; the session keeps running.
    JoinFailed {
        reason: String,
    },
    /// The network thread died; the session is offline from here on.
    Fatal {
        reason: String,
    },
}

/// Sends [`NetCommand`]s to the network thread; synchronous and
/// non-blocking, safe to call from any game system. Aliased so dependents
/// don't need their own tokio dependency.
pub type CommandSender = tokio::sync::mpsc::UnboundedSender<NetCommand>;

/// Handle held by the client; dropping it shuts the network thread down.
pub struct NetHandle {
    pub commands: CommandSender,
    pub events: std::sync::mpsc::Receiver<NetEvent>,
}

/// Start the network thread: a dedicated OS thread running a current-thread
/// tokio runtime with the libp2p swarm loop.
pub fn spawn(config: NetConfig) -> NetHandle {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (evt_tx, evt_rx) = std::sync::mpsc::channel();

    std::thread::Builder::new()
        .name("civora-net".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build tokio runtime");
            if let Err(reason) = runtime.block_on(run(config, cmd_rx, evt_tx.clone())) {
                let _ = evt_tx.send(NetEvent::Fatal { reason });
            }
        })
        .expect("spawn civora-net thread");

    NetHandle {
        commands: cmd_tx,
        events: evt_rx,
    }
}
