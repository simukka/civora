//! The swarm event loop: lobby membership, the join/serve snapshot state
//! machines, and live gossip relay.
//!
//! The loop is deliberately dumb about game state: it never owns the world
//! or decides whether peers diverged. It verifies transferred logs and
//! snapshots structurally (signatures, sequence order, content hash) and
//! forwards everything else to the client, which owns the sim.

use std::collections::HashMap;

use civora_governance::{Cid, MAX_BLOB_BYTES};
use civora_identity::{ActionLog, PlayerId};
use futures::StreamExt;
use libp2p::request_response::{self, OutboundRequestId, ResponseChannel};
use libp2p::swarm::SwarmEvent;
use libp2p::{Multiaddr, PeerId, Swarm, gossipsub, mdns, noise, tcp, yamux};

use crate::behaviour::{Behaviour, BehaviourEvent};
use crate::peer::{keypair_from_seed, player_id_of};
use crate::wire::{
    self, FetchRequest, FetchResponse, GossipMsg, PROTO_VERSION, RejectReason, SyncRequest,
    SyncResponse,
};
use crate::{NetCommand, NetConfig, NetEvent, SessionMode};

/// Gossip received while still syncing is buffered and flushed after
/// `WorldSync`; the log's seq gate drops entries the snapshot already
/// contained. The cap only guards memory — a join takes well under a second.
const MAX_BUFFERED_ACTIONS: usize = 10_000;

/// Run the swarm loop until the client drops its handle.
///
/// `Err` is returned only for startup failures (bad config, no transport);
/// runtime problems surface as [`NetEvent`]s instead.
pub async fn run(
    config: NetConfig,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<NetCommand>,
    evt_tx: std::sync::mpsc::Sender<NetEvent>,
) -> Result<(), String> {
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair_from_seed(config.seed))
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .map_err(|err| format!("tcp transport: {err}"))?
        .with_behaviour(
            |key| -> Result<Behaviour, Box<dyn std::error::Error + Send + Sync>> {
                Ok(Behaviour::new(key, config.enable_mdns)?)
            },
        )
        .map_err(|err| format!("behaviour: {err}"))?
        .build();

    let actions_topic = gossipsub::IdentTopic::new(config.cell.actions_topic());
    let state_topic = gossipsub::IdentTopic::new(config.cell.state_topic());
    let proposals_topic = gossipsub::IdentTopic::new(config.cell.proposals_topic());
    for topic in [&actions_topic, &state_topic, &proposals_topic] {
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(topic)
            .map_err(|err| format!("subscribe {}: {err}", topic.hash()))?;
    }

    // Everyone listens (late peers dial us to form a full mesh), everyone
    // runs mDNS; only the mode decides whether we serve or request joins.
    swarm
        .listen_on(
            "/ip4/0.0.0.0/tcp/0"
                .parse()
                .expect("static multiaddr parses"),
        )
        .map_err(|err| format!("listen: {err}"))?;

    let mut ctx = EventLoop {
        evt_tx,
        actions_topic,
        proposals_topic,
        cell: config.cell.clone(),
        live: matches!(config.mode, SessionMode::Host),
        awaiting_join: matches!(config.mode, SessionMode::Join { .. }),
        join_request: None,
        buffered: Vec::new(),
        players: HashMap::new(),
        pending_snapshots: HashMap::new(),
        pending_blobs: HashMap::new(),
        pending_fetches: HashMap::new(),
        next_request_id: 0,
    };

    if let SessionMode::Join { dial: Some(addr) } = &config.mode {
        let addr: Multiaddr = addr
            .parse()
            .map_err(|err| format!("--join address {addr:?}: {err}"))?;
        swarm
            .dial(addr)
            .map_err(|err| format!("dial --join address: {err}"))?;
    }

    loop {
        tokio::select! {
            command = cmd_rx.recv() => match command {
                // The client dropped its handle; shut down.
                None => return Ok(()),
                Some(command) => {
                    if ctx.on_command(&mut swarm, command).is_err() {
                        return Ok(()); // client hung up
                    }
                }
            },
            event = swarm.select_next_some() => {
                if ctx.on_swarm_event(&mut swarm, event).is_err() {
                    return Ok(()); // client hung up
                }
            }
        }
    }
}

/// Sending on a closed event channel means the client is gone; bubble that
/// up as `Err(())` so the loop exits cleanly.
type SendResult = Result<(), ()>;

struct EventLoop {
    evt_tx: std::sync::mpsc::Sender<NetEvent>,
    actions_topic: gossipsub::IdentTopic,
    proposals_topic: gossipsub::IdentTopic,
    cell: wire::CellRef,
    /// Hosts start live; joiners go live after their first `WorldSync`.
    live: bool,
    /// A joiner that has not yet sent its initial join request.
    awaiting_join: bool,
    /// The in-flight join or resync request, if any.
    join_request: Option<OutboundRequestId>,
    buffered: Vec<civora_identity::SignedAction>,
    players: HashMap<PeerId, PlayerId>,
    pending_snapshots: HashMap<u64, ResponseChannel<SyncResponse>>,
    /// Inbound blob requests awaiting a [`NetCommand::ProvideBlob`], keyed by the
    /// same `next_request_id` counter snapshots use.
    pending_blobs: HashMap<u64, ResponseChannel<FetchResponse>>,
    /// Outbound blob fetches in flight, tracking the target cid and which peers
    /// have already been tried so a failure can retry the next one.
    pending_fetches: HashMap<OutboundRequestId, FetchState>,
    next_request_id: u64,
}

/// State for one outbound blob fetch across retries.
struct FetchState {
    cid: Cid,
    /// Peers already asked (in order), so retries pick a fresh one.
    tried: Vec<PeerId>,
}

impl EventLoop {
    fn emit(&self, event: NetEvent) -> SendResult {
        self.evt_tx.send(event).map_err(|_| ())
    }

    fn on_command(&mut self, swarm: &mut Swarm<Behaviour>, command: NetCommand) -> SendResult {
        match command {
            NetCommand::PublishAction(signed) => {
                let mut payload = Vec::new();
                GossipMsg::Action(signed).encode(&mut payload);
                self.publish(swarm, self.actions_topic.clone(), payload);
            }
            NetCommand::PublishBeacon(beacon) => {
                let topic = gossipsub::IdentTopic::new(self.cell.state_topic());
                let mut payload = Vec::new();
                GossipMsg::Beacon(beacon).encode(&mut payload);
                self.publish(swarm, topic, payload);
            }
            NetCommand::PublishProposal(signed) => {
                let mut payload = Vec::new();
                GossipMsg::Proposal(signed).encode(&mut payload);
                self.publish(swarm, self.proposals_topic.clone(), payload);
            }
            NetCommand::PublishVote(signed) => {
                let mut payload = Vec::new();
                GossipMsg::Vote(signed).encode(&mut payload);
                self.publish(swarm, self.proposals_topic.clone(), payload);
            }
            NetCommand::PublishCertificate(signed) => {
                let mut payload = Vec::new();
                GossipMsg::Certificate(signed).encode(&mut payload);
                self.publish(swarm, self.proposals_topic.clone(), payload);
            }
            NetCommand::ProvideSnapshot {
                request_id,
                snapshot,
            } => {
                let Some(channel) = self.pending_snapshots.remove(&request_id) else {
                    return Ok(()); // requester disconnected meanwhile
                };
                let response = SyncResponse::Accept {
                    proto: PROTO_VERSION,
                    cell: self.cell.clone(),
                    content_hash: snapshot.content_hash,
                    log: snapshot.log,
                    chunks: snapshot.chunks,
                    ledger: snapshot.ledger,
                    open_proposals: snapshot.open_proposals,
                    open_votes: snapshot.open_votes,
                };
                let _ = swarm.behaviour_mut().sync.send_response(channel, response);
            }
            NetCommand::FetchBlob { cid } => {
                // Coalesce: one in-flight fetch per cid. A client retry timer
                // re-requests, so a dropped duplicate is never lost.
                if self.pending_fetches.values().any(|f| f.cid == cid) {
                    tracing::debug!("fetch for {cid} already in flight; dropping duplicate");
                    return Ok(());
                }
                return self.start_fetch(swarm, cid, Vec::new(), None);
            }
            NetCommand::ProvideBlob { request_id, bytes } => {
                let Some(channel) = self.pending_blobs.remove(&request_id) else {
                    return Ok(()); // requester disconnected meanwhile
                };
                let response = match bytes {
                    Some(bytes) if bytes.len() <= MAX_BLOB_BYTES => FetchResponse::Blob { bytes },
                    // Missing, or an oversize blob we refuse to frame.
                    _ => FetchResponse::NotFound,
                };
                let _ = swarm.behaviour_mut().fetch.send_response(channel, response);
            }
            NetCommand::Resync { preferred } => {
                let target = preferred
                    .and_then(|player| {
                        self.players
                            .iter()
                            .find(|(_, p)| **p == player)
                            .map(|(peer, _)| *peer)
                    })
                    .or_else(|| self.players.keys().next().copied());
                match target {
                    Some(peer) => self.send_join(swarm, peer),
                    None => {
                        return self.emit(NetEvent::JoinFailed {
                            reason: "no connected peers to resync from".to_owned(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn publish(
        &self,
        swarm: &mut Swarm<Behaviour>,
        topic: gossipsub::IdentTopic,
        payload: Vec<u8>,
    ) {
        if let Err(err) = swarm.behaviour_mut().gossipsub.publish(topic, payload) {
            // InsufficientPeers while alone in the lobby is normal.
            tracing::debug!("gossipsub publish failed: {err}");
        }
    }

    fn send_join(&mut self, swarm: &mut Swarm<Behaviour>, peer: PeerId) {
        let request = SyncRequest::join(self.cell.clone());
        self.join_request = Some(swarm.behaviour_mut().sync.send_request(&peer, request));
        self.awaiting_join = false;
    }

    /// Send a blob fetch to the first connected peer not yet `tried`, recording
    /// it as in flight. With no untried peer left the fetch is exhausted:
    /// emit [`NetEvent::BlobFetchFailed`] carrying `last_reason` (or a
    /// no-peers message). The client's retry timer re-requests exhausted cids.
    fn start_fetch(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        cid: Cid,
        mut tried: Vec<PeerId>,
        last_reason: Option<String>,
    ) -> SendResult {
        let next = self
            .players
            .keys()
            .find(|peer| !tried.contains(peer))
            .copied();
        match next {
            Some(peer) => {
                let request_id = swarm
                    .behaviour_mut()
                    .fetch
                    .send_request(&peer, FetchRequest::Blob { cid });
                tried.push(peer);
                self.pending_fetches
                    .insert(request_id, FetchState { cid, tried });
                Ok(())
            }
            None => self.emit(NetEvent::BlobFetchFailed {
                cid,
                reason: last_reason
                    .unwrap_or_else(|| "no connected peers to fetch from".to_owned()),
            }),
        }
    }

    fn on_fetch(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        event: request_response::Event<FetchRequest, FetchResponse>,
    ) -> SendResult {
        match event {
            // Inbound: a peer wants a blob. Round-trip through the client (which
            // owns the store) like snapshots do; no live gate — the store exists
            // before the world syncs.
            request_response::Event::Message {
                message:
                    request_response::Message::Request {
                        request, channel, ..
                    },
                ..
            } => {
                let FetchRequest::Blob { cid } = request;
                let request_id = self.next_request_id;
                self.next_request_id += 1;
                self.pending_blobs.insert(request_id, channel);
                self.emit(NetEvent::BlobRequested { request_id, cid })?;
            }
            // Outbound: a peer answered our fetch.
            request_response::Event::Message {
                message:
                    request_response::Message::Response {
                        request_id,
                        response,
                    },
                ..
            } => {
                let Some(state) = self.pending_fetches.remove(&request_id) else {
                    return Ok(());
                };
                match response {
                    // The net layer owns the content-hash check (the house
                    // split); a mismatch is treated exactly like NotFound.
                    FetchResponse::Blob { bytes } if Cid::of(&bytes) == state.cid => {
                        self.emit(NetEvent::BlobFetched {
                            cid: state.cid,
                            bytes,
                        })?;
                    }
                    FetchResponse::Blob { .. } => {
                        return self.start_fetch(
                            swarm,
                            state.cid,
                            state.tried,
                            Some("hash mismatch".to_owned()),
                        );
                    }
                    FetchResponse::NotFound => {
                        return self.start_fetch(
                            swarm,
                            state.cid,
                            state.tried,
                            Some("not found".to_owned()),
                        );
                    }
                }
            }
            // Outbound failure — includes peers that speak no `/civora/fetch/1`
            // (`UnsupportedProtocols`). Retry the next peer.
            request_response::Event::OutboundFailure {
                request_id, error, ..
            } => {
                if let Some(state) = self.pending_fetches.remove(&request_id) {
                    return self.start_fetch(
                        swarm,
                        state.cid,
                        state.tried,
                        Some(format!("fetch request failed: {error}")),
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn on_swarm_event(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        event: SwarmEvent<BehaviourEvent>,
    ) -> SendResult {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                let peer_id = *swarm.local_peer_id();
                self.emit(NetEvent::ListeningOn {
                    addr: format!("{address}/p2p/{peer_id}"),
                })?;
            }
            SwarmEvent::ConnectionEstablished {
                peer_id,
                endpoint,
                num_established,
                ..
            } => {
                if num_established.get() > 1 {
                    return Ok(()); // roster already knows this peer
                }
                // PeerId == PlayerId is the lobby's admission rule: a peer
                // whose id is not an inlined Ed25519 key is not a Civora
                // client and gets disconnected.
                let Some(player) = player_id_of(&peer_id) else {
                    let _ = swarm.disconnect_peer_id(peer_id);
                    return Ok(());
                };
                self.players.insert(peer_id, player);
                self.emit(NetEvent::PeerConnected {
                    player,
                    addr: endpoint.get_remote_address().to_string(),
                })?;
                if self.awaiting_join && self.join_request.is_none() {
                    self.send_join(swarm, peer_id);
                }
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established,
                ..
            } => {
                if num_established == 0
                    && let Some(player) = self.players.remove(&peer_id)
                {
                    self.emit(NetEvent::PeerDisconnected { player })?;
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
                for (peer_id, addr) in peers {
                    if !swarm.is_connected(&peer_id) {
                        let _ = swarm.dial(addr);
                    }
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                message,
                ..
            })) => {
                return self.on_gossip(message);
            }
            SwarmEvent::Behaviour(BehaviourEvent::Sync(event)) => {
                return self.on_sync(swarm, event);
            }
            SwarmEvent::Behaviour(BehaviourEvent::Fetch(event)) => {
                return self.on_fetch(swarm, event);
            }
            _ => {}
        }
        Ok(())
    }

    fn on_gossip(&mut self, message: gossipsub::Message) -> SendResult {
        match GossipMsg::decode(&message.data) {
            Some(GossipMsg::Action(signed)) => {
                if self.live {
                    self.emit(NetEvent::RemoteAction(signed))?;
                } else if self.buffered.len() < MAX_BUFFERED_ACTIONS {
                    self.buffered.push(signed);
                }
            }
            Some(GossipMsg::Beacon(beacon)) => {
                // Signed gossipsub messages always carry their author.
                let Some(from) = message.source.as_ref().and_then(player_id_of) else {
                    return Ok(());
                };
                if self.live {
                    self.emit(NetEvent::RemoteBeacon { from, beacon })?;
                }
            }
            // Governance gossip bypasses the live gate: unlike actions it
            // carries no ordering the snapshot could pre-empt, and the
            // client's store insertion is idempotent.
            Some(GossipMsg::Proposal(signed)) => {
                self.emit(NetEvent::RemoteProposal(signed))?;
            }
            Some(GossipMsg::Vote(signed)) => {
                self.emit(NetEvent::RemoteVote(signed))?;
            }
            Some(GossipMsg::Certificate(signed)) => {
                self.emit(NetEvent::RemoteCertificate(signed))?;
            }
            None => tracing::debug!("dropping malformed gossip message"),
        }
        Ok(())
    }

    fn on_sync(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        event: request_response::Event<SyncRequest, SyncResponse>,
    ) -> SendResult {
        match event {
            request_response::Event::Message {
                message:
                    request_response::Message::Request {
                        request, channel, ..
                    },
                ..
            } => {
                let SyncRequest::Join {
                    proto,
                    chunk_size,
                    cell,
                } = request;
                let reject = if proto != PROTO_VERSION {
                    Some(RejectReason::ProtoMismatch)
                } else if chunk_size != civora_sim::CHUNK_SIZE as u32 {
                    Some(RejectReason::ChunkSizeMismatch)
                } else if cell != self.cell {
                    Some(RejectReason::UnknownCell)
                } else if !self.live {
                    Some(RejectReason::NotReady)
                } else {
                    None
                };
                if let Some(reason) = reject {
                    let _ = swarm
                        .behaviour_mut()
                        .sync
                        .send_response(channel, SyncResponse::Reject { reason });
                    return Ok(());
                }
                // The client owns the world, so the snapshot round-trips
                // through it instead of this loop holding any world state.
                let request_id = self.next_request_id;
                self.next_request_id += 1;
                self.pending_snapshots.insert(request_id, channel);
                self.emit(NetEvent::SnapshotRequested { request_id })?;
            }
            request_response::Event::Message {
                message:
                    request_response::Message::Response {
                        request_id,
                        response,
                    },
                ..
            } => {
                if self.join_request != Some(request_id) {
                    return Ok(());
                }
                self.join_request = None;
                return self.on_join_response(response);
            }
            request_response::Event::OutboundFailure {
                request_id, error, ..
            } if self.join_request == Some(request_id) => {
                self.join_request = None;
                self.emit(NetEvent::JoinFailed {
                    reason: format!("sync request failed: {error}"),
                })?;
            }
            _ => {}
        }
        Ok(())
    }

    fn on_join_response(&mut self, response: SyncResponse) -> SendResult {
        let (proto, cell, content_hash, log_entries, chunks, ledger, open_proposals, open_votes) =
            match response {
                SyncResponse::Reject { reason } => {
                    return self.emit(NetEvent::JoinFailed {
                        reason: format!("peer rejected join: {reason:?}"),
                    });
                }
                SyncResponse::Accept {
                    proto,
                    cell,
                    content_hash,
                    log,
                    chunks,
                    ledger,
                    open_proposals,
                    open_votes,
                } => (
                    proto,
                    cell,
                    content_hash,
                    log,
                    chunks,
                    ledger,
                    open_proposals,
                    open_votes,
                ),
            };

        if proto != PROTO_VERSION || cell != self.cell {
            return self.emit(NetEvent::JoinFailed {
                reason: "peer answered for a different protocol or cell".to_owned(),
            });
        }

        // Rebuild the log through the kernel gate: every entry re-verifies
        // its signature and per-author sequence on append.
        let mut log = ActionLog::new();
        for entry in log_entries {
            if let Err(err) = log.append(entry) {
                return self.emit(NetEvent::JoinFailed {
                    reason: format!("transferred log failed verification: {err}"),
                });
            }
        }

        let Some(world) = wire::world_from_chunks(&chunks) else {
            return self.emit(NetEvent::JoinFailed {
                reason: "transferred snapshot contained an invalid chunk".to_owned(),
            });
        };
        if world.content_hash() != content_hash {
            return self.emit(NetEvent::JoinFailed {
                reason: "snapshot does not match its advertised content hash".to_owned(),
            });
        }

        self.live = true;
        self.emit(NetEvent::WorldSync {
            world,
            log,
            content_hash,
        })?;
        // Governance state rides the same response. Re-emit it as ordinary
        // gossip events, in dependency order, so the client re-verifies each
        // through its existing gates: a proposal before the certificate that
        // finalizes it, then open proposals before their ballots. No
        // verification happens in the net layer (the house split).
        for (proposal, certificate) in ledger {
            self.emit(NetEvent::RemoteProposal(Box::new(proposal)))?;
            self.emit(NetEvent::RemoteCertificate(Box::new(certificate)))?;
        }
        for proposal in open_proposals {
            self.emit(NetEvent::RemoteProposal(Box::new(proposal)))?;
        }
        for vote in open_votes {
            self.emit(NetEvent::RemoteVote(vote))?;
        }
        // Gossip that raced the snapshot: the client's log seq gate drops
        // whatever the snapshot already contained.
        for signed in std::mem::take(&mut self.buffered) {
            self.emit(NetEvent::RemoteAction(signed))?;
        }
        Ok(())
    }
}
