//! Two-node lobby + world sync integration test.
//!
//! Runs two real network threads (real TCP on 127.0.0.1, real Noise, real
//! gossipsub) with the test playing each node's "client" role: answering
//! snapshot requests, appending remote actions through the kernel gate, and
//! applying them with `tick::step`. mDNS stays off so the test is
//! deterministic and CI-safe; the joiner direct-dials the host.

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use civora_governance::{
    Cid, Ledger, LedgerEntry, Proposal, ProposalId, ProposalKind, RollbackPlan, SignedCertificate,
    SignedProposal, SignedVote, Vote, VoteChoice,
};
use civora_identity::{ActionLog, Identity, PlayerId, VerifyError};
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

/// A network node plus the sim and governance state its "client" would own.
struct TestNode {
    net: NetHandle,
    identity: Identity,
    world: VoxelWorld,
    log: ActionLog,
    next_seq: u64,
    /// Accepted-proposal ledger, mirroring the client's `LedgerStore`.
    ledger: Ledger,
    /// Open proposals and their ballots the client would serve on a join.
    open_proposals: Vec<SignedProposal>,
    open_votes: Vec<SignedVote>,
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
            ledger: Ledger::default(),
            open_proposals: Vec::new(),
            open_votes: Vec::new(),
        }
    }

    fn player_id(&self) -> PlayerId {
        self.identity.player_id()
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
        let ledger = self
            .ledger
            .entries()
            .iter()
            .map(|e| (e.proposal.clone(), e.certificate.clone()))
            .collect();
        self.net
            .commands
            .send(NetCommand::ProvideSnapshot {
                request_id,
                snapshot: Snapshot {
                    content_hash: self.world.content_hash(),
                    log: self.log.entries().to_vec(),
                    chunks: snapshot_chunks(&self.world),
                    ledger,
                    open_proposals: self.open_proposals.clone(),
                    open_votes: self.open_votes.clone(),
                },
            })
            .unwrap();
    }
}

/// A minimal valid asset-patch proposal authored by `author`. The
/// `activation_epoch` doubles as the voting-window close; tests certify with
/// `accepted_epoch == activation_epoch` (window already closed) since the net
/// layer has no epoch logic of its own.
fn asset_proposal(author: &Identity, activation_epoch: u64, seed: u8) -> Proposal {
    Proposal {
        kind: ProposalKind::AssetPatch,
        author_public_key: author.player_id(),
        git_commit_hash: [seed; 20],
        source_bundle_cid: test_cid(seed, 0),
        build_manifest_cid: test_cid(seed, 1),
        wasm_module_cids: vec![],
        asset_cids: vec![test_cid(seed, 2)],
        migration_cids: vec![],
        governance_change: None,
        test_results_cid: test_cid(seed, 3),
        activation_epoch,
        rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
    }
}

fn yes_ballot(voter: &Identity, proposal_id: ProposalId) -> SignedVote {
    SignedVote::sign(
        voter,
        Vote {
            proposal_id,
            voter: voter.player_id(),
            choice: VoteChoice::Yes,
        },
    )
}

/// A distinct 32-byte cid; `i` in big-endian keeps lists strictly ascending.
fn test_cid(list: u8, i: u16) -> Cid {
    let mut bytes = [0u8; 32];
    bytes[0] = list;
    bytes[1..3].copy_from_slice(&i.to_be_bytes());
    Cid(bytes)
}

#[test]
fn proposals_and_votes_gossip() {
    // --- Two live nodes, no world history needed.
    let mut host = TestNode::start(1, SessionMode::Host);
    host.world = VoxelWorld::flat(1);
    let host_addr = wait_for(&host.net.events, "host listen addr", |event| match event {
        NetEvent::ListeningOn { addr } if addr.contains("127.0.0.1") => Some(addr),
        _ => None,
    });
    let joiner = TestNode::start(
        2,
        SessionMode::Join {
            dial: Some(host_addr),
        },
    );
    let request_id = wait_for(&host.net.events, "snapshot request", |event| match event {
        NetEvent::SnapshotRequested { request_id } => Some(request_id),
        _ => None,
    });
    host.serve_snapshot(request_id);
    wait_for(&joiner.net.events, "world sync", |event| match event {
        NetEvent::WorldSync { .. } => Some(()),
        NetEvent::JoinFailed { reason } => panic!("join failed: {reason}"),
        _ => None,
    });
    // Give the gossipsub subscription exchange a moment to form the mesh.
    std::thread::sleep(Duration::from_millis(2000));

    // --- Host publishes a proposal with all three cid lists full. Encoded
    // it is well over gossipsub's 64 KiB default max_transmit_size, pinning
    // the raised limit.
    let full_list = |list: u8| (0..1024).map(|i| test_cid(list, i)).collect::<Vec<_>>();
    let proposal = Proposal {
        kind: ProposalKind::GameplayCode,
        author_public_key: host.identity.player_id(),
        git_commit_hash: [0xAA; 20],
        source_bundle_cid: test_cid(9, 0),
        build_manifest_cid: test_cid(9, 1),
        wasm_module_cids: full_list(1),
        asset_cids: full_list(2),
        migration_cids: full_list(3),
        governance_change: None,
        test_results_cid: test_cid(9, 2),
        activation_epoch: 1000,
        rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
    };
    proposal.validate().expect("test proposal is valid");
    let signed = SignedProposal::sign(&host.identity, proposal);
    let mut encoded = Vec::new();
    signed.encode(&mut encoded);
    assert!(
        encoded.len() > 64 * 1024,
        "proposal must exceed the gossipsub default limit to pin the raise \
         (got {} bytes)",
        encoded.len()
    );
    host.net
        .commands
        .send(NetCommand::PublishProposal(Box::new(signed.clone())))
        .unwrap();

    let received = wait_for(
        &joiner.net.events,
        "gossiped proposal",
        |event| match event {
            NetEvent::RemoteProposal(signed) => Some(signed),
            _ => None,
        },
    );
    assert_eq!(*received, signed);
    received.verify().expect("proposal signature verifies");
    received.proposal.validate().expect("proposal validates");
    assert_eq!(received.proposal_id(), signed.proposal_id());

    // --- Joiner votes yes; the host receives and verifies the ballot.
    let ballot = SignedVote::sign(
        &joiner.identity,
        Vote {
            proposal_id: signed.proposal_id(),
            voter: joiner.identity.player_id(),
            choice: VoteChoice::Yes,
        },
    );
    joiner
        .net
        .commands
        .send(NetCommand::PublishVote(ballot))
        .unwrap();

    let received = wait_for(&host.net.events, "gossiped vote", |event| match event {
        NetEvent::RemoteVote(signed) => Some(signed),
        _ => None,
    });
    assert_eq!(received, ballot);
    received.verify().expect("vote signature verifies");
    assert_eq!(received.vote.proposal_id, signed.proposal_id());
    assert_eq!(received.vote.choice, VoteChoice::Yes);
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

/// Bring a joiner online and synced against a host serving `host.world`.
/// Returns once the joiner has received `WorldSync`; both `world` fields and
/// the mesh are ready for gossip.
fn connect_and_sync(host: &mut TestNode, joiner: &mut TestNode) {
    let request_id = wait_for(&host.net.events, "snapshot request", |event| match event {
        NetEvent::SnapshotRequested { request_id } => Some(request_id),
        _ => None,
    });
    host.serve_snapshot(request_id);
    let world = wait_for(&joiner.net.events, "world sync", |event| match event {
        NetEvent::WorldSync { world, .. } => Some(world),
        NetEvent::JoinFailed { reason } => panic!("join failed: {reason}"),
        _ => None,
    });
    joiner.world = world;
    // Let the gossipsub subscription exchange form the mesh before publishing.
    std::thread::sleep(Duration::from_millis(2000));
}

#[test]
fn certificate_gossip_reaches_both_ledgers() {
    let mut host = TestNode::start(1, SessionMode::Host);
    host.world = VoxelWorld::flat(1);
    let host_addr = wait_for(&host.net.events, "host listen addr", |event| match event {
        NetEvent::ListeningOn { addr } if addr.contains("127.0.0.1") => Some(addr),
        _ => None,
    });
    let mut joiner = TestNode::start(
        2,
        SessionMode::Join {
            dial: Some(host_addr),
        },
    );
    connect_and_sync(&mut host, &mut joiner);

    // The host authors a proposal whose window has already closed
    // (activation_epoch 0) and gossips it; the joiner receives it.
    let proposal = asset_proposal(&host.identity, 0, 0x10);
    let signed = SignedProposal::sign(&host.identity, proposal.clone());
    host.net
        .commands
        .send(NetCommand::PublishProposal(Box::new(signed.clone())))
        .unwrap();
    let joiner_proposal = wait_for(
        &joiner.net.events,
        "gossiped proposal",
        |event| match event {
            NetEvent::RemoteProposal(signed) => Some(signed),
            _ => None,
        },
    );
    assert_eq!(*joiner_proposal, signed);

    // Both peers vote yes. The host, holding both ballots, certifies over the
    // two-member roster and appends the accepted entry to its ledger.
    let id = signed.proposal_id();
    let mut roster = vec![host.player_id(), joiner.player_id()];
    roster.sort();
    let ballots: BTreeMap<PlayerId, SignedVote> = [
        (host.player_id(), yes_ballot(&host.identity, id)),
        (joiner.player_id(), yes_ballot(&joiner.identity, id)),
    ]
    .into();
    let certificate =
        SignedCertificate::certify(&host.identity, &proposal, &roster, &ballots, 1, 0)
            .expect("two yes votes over a roster of two clears majority");
    let entry = LedgerEntry {
        proposal: signed.clone(),
        certificate: certificate.clone(),
    };
    assert_eq!(host.ledger.append(entry.clone()), Ok(true));

    // The host gossips the certificate; the joiner receives it and appends via
    // its own ledger gate (which re-verifies every signature).
    host.net
        .commands
        .send(NetCommand::PublishCertificate(Box::new(
            certificate.clone(),
        )))
        .unwrap();
    let joiner_cert = wait_for(
        &joiner.net.events,
        "gossiped certificate",
        |event| match event {
            NetEvent::RemoteCertificate(signed) => Some(signed),
            _ => None,
        },
    );
    assert_eq!(*joiner_cert, certificate);
    let joiner_entry = LedgerEntry {
        proposal: (*joiner_proposal).clone(),
        certificate: *joiner_cert,
    };
    assert_eq!(joiner.ledger.append(joiner_entry), Ok(true));

    // The accepted set converged on both peers, and a re-seen certificate is a
    // no-op (first valid certificate wins).
    assert!(host.ledger.contains(&id));
    assert!(joiner.ledger.contains(&id));
    assert_eq!(host.ledger.append(entry), Ok(false));
}

#[test]
fn join_syncs_governance_state() {
    let mut host = TestNode::start(1, SessionMode::Host);
    host.world = VoxelWorld::flat(1);

    // Pre-load the host with one accepted entry (host as sole roster) ...
    let accepted = asset_proposal(&host.identity, 0, 0x20);
    let accepted_signed = SignedProposal::sign(&host.identity, accepted.clone());
    let accepted_id = accepted_signed.proposal_id();
    let ballots: BTreeMap<PlayerId, SignedVote> =
        [(host.player_id(), yes_ballot(&host.identity, accepted_id))].into();
    let certificate = SignedCertificate::certify(
        &host.identity,
        &accepted,
        &[host.player_id()],
        &ballots,
        1,
        0,
    )
    .expect("sole yes voter self-accepts");
    host.ledger
        .append(LedgerEntry {
            proposal: accepted_signed.clone(),
            certificate: certificate.clone(),
        })
        .unwrap();

    // ... and one still-open proposal with a ballot.
    let open = asset_proposal(&host.identity, 1000, 0x30);
    let open_signed = SignedProposal::sign(&host.identity, open.clone());
    let open_id = open_signed.proposal_id();
    let open_vote = yes_ballot(&host.identity, open_id);
    host.open_proposals.push(open_signed.clone());
    host.open_votes.push(open_vote);

    let host_addr = wait_for(&host.net.events, "host listen addr", |event| match event {
        NetEvent::ListeningOn { addr } if addr.contains("127.0.0.1") => Some(addr),
        _ => None,
    });
    let mut joiner = TestNode::start(
        2,
        SessionMode::Join {
            dial: Some(host_addr),
        },
    );

    let request_id = wait_for(&host.net.events, "snapshot request", |event| match event {
        NetEvent::SnapshotRequested { request_id } => Some(request_id),
        _ => None,
    });
    host.serve_snapshot(request_id);
    wait_for(&joiner.net.events, "world sync", |event| match event {
        NetEvent::WorldSync { .. } => Some(()),
        NetEvent::JoinFailed { reason } => panic!("join failed: {reason}"),
        _ => None,
    });

    // The governance payload rides the join response as ordinary gossip events
    // after `WorldSync`: a proposal before the certificate that finalizes it,
    // then the open proposal and its ballot. Rebuild the joiner's state through
    // the same gates the client uses.
    let mut proposals: HashMap<ProposalId, SignedProposal> = HashMap::new();
    let mut votes: Vec<SignedVote> = Vec::new();
    loop {
        let event = joiner
            .net
            .events
            .recv_timeout(EVENT_TIMEOUT)
            .expect("governance events after world sync");
        match event {
            NetEvent::RemoteProposal(signed) => {
                signed.verify().expect("proposal verifies");
                proposals.insert(signed.proposal_id(), *signed);
            }
            NetEvent::RemoteCertificate(signed) => {
                let proposal = proposals
                    .get(&signed.certificate.proposal_id)
                    .expect("certificate's proposal arrived first")
                    .clone();
                assert_eq!(
                    joiner.ledger.append(LedgerEntry {
                        proposal,
                        certificate: *signed,
                    }),
                    Ok(true)
                );
            }
            NetEvent::RemoteVote(signed) => {
                signed.verify().expect("vote verifies");
                votes.push(signed);
                break; // the ballot is last in dependency order
            }
            _ => {}
        }
    }

    // The accepted entry landed in the joiner's ledger; the open proposal and
    // its ballot arrived but are not accepted.
    assert!(joiner.ledger.contains(&accepted_id));
    assert!(!joiner.ledger.contains(&open_id));
    assert!(proposals.contains_key(&open_id), "open proposal synced");
    assert_eq!(votes.len(), 1);
    assert_eq!(votes[0].vote.proposal_id, open_id, "open ballot synced");
}
