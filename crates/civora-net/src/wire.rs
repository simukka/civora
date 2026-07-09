//! Canonical wire encodings for the Civora P2P protocol.
//!
//! Same house rules as [`civora_sim::Action::encode`]: every message has
//! exactly one encoding. Decoders reject unknown tags, truncated input,
//! trailing bytes, and non-canonical orderings, so decode(encode(m))
//! round-trips and nothing else parses.

use civora_governance::{SignedCertificate, SignedProposal, SignedVote};
use civora_identity::{PlayerId, SignedAction};
use civora_sim::{CHUNK_SIZE, Chunk, ChunkPos, VoxelWorld};

/// Protocol version, embedded in gossip topic names and the join handshake.
/// Bump on any breaking wire or sim-semantics change. Version 2 adds the
/// governance join-sync payload to [`SyncResponse::Accept`] and the
/// certificate gossip variant — a breaking response change.
pub const PROTO_VERSION: u32 = 2;

/// Hard cap on an encoded sync request (a join handshake is tiny).
pub const MAX_REQUEST_BYTES: usize = 4 * 1024;

/// Hard cap on an encoded sync response (whole-world snapshot + log).
/// Whole-world transfer is a single-cell simplification; cell partitioning
/// replaces it before worlds outgrow this.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// Decode caps on the governance payload a join response carries. A worst-case
/// ledger entry is a full certificate (~131 KiB) plus its proposal; 64 of those
/// is ~34 MiB, safely under [`MAX_RESPONSE_BYTES`] alongside the world. Larger
/// ledgers wait for announce-then-fetch in the patch-pack milestone.
pub const MAX_SYNC_LEDGER_ENTRIES: usize = 64;
pub const MAX_SYNC_OPEN_PROPOSALS: usize = 64;
pub const MAX_SYNC_VOTES: usize = 8192;

/// Bytes in one transferred chunk payload ([`Chunk::block_bytes`] order).
pub const CHUNK_BYTES: usize = (CHUNK_SIZE as usize).pow(3);

fn take(bytes: &[u8], n: usize) -> Option<(&[u8], &[u8])> {
    (bytes.len() >= n).then(|| bytes.split_at(n))
}

fn u16_le(bytes: &[u8]) -> Option<(u16, &[u8])> {
    let (raw, rest) = take(bytes, 2)?;
    Some((u16::from_le_bytes(raw.try_into().unwrap()), rest))
}

fn u32_le(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let (raw, rest) = take(bytes, 4)?;
    Some((u32::from_le_bytes(raw.try_into().unwrap()), rest))
}

fn u64_le(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let (raw, rest) = take(bytes, 8)?;
    Some((u64::from_le_bytes(raw.try_into().unwrap()), rest))
}

fn i32_le(bytes: &[u8]) -> Option<(i32, &[u8])> {
    let (raw, rest) = take(bytes, 4)?;
    Some((i32::from_le_bytes(raw.try_into().unwrap()), rest))
}

/// Address of one simulation cell inside a realm.
///
/// This milestone runs a single cell (the whole world, `genesis/0`), but the
/// reference travels in every topic name and sync message so cell
/// partitioning later is a new topic per cell, not a wire format break.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CellRef {
    pub realm: String,
    pub cell: u64,
}

impl CellRef {
    /// The one cell that exists in this milestone.
    pub fn genesis() -> Self {
        Self {
            realm: "genesis".to_owned(),
            cell: 0,
        }
    }

    /// Gossipsub topic for live signed actions in this cell.
    pub fn actions_topic(&self) -> String {
        format!(
            "civora/{PROTO_VERSION}/{}/{}/actions",
            self.realm, self.cell
        )
    }

    /// Gossipsub topic for periodic state beacons in this cell.
    pub fn state_topic(&self) -> String {
        format!("civora/{PROTO_VERSION}/{}/{}/state", self.realm, self.cell)
    }

    /// Gossipsub topic for governance traffic (signed proposals and votes)
    /// in this cell.
    pub fn proposals_topic(&self) -> String {
        format!(
            "civora/{PROTO_VERSION}/{}/{}/proposals",
            self.realm, self.cell
        )
    }

    /// `realm_len (u8) || realm bytes || cell (u64 LE)`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        assert!(self.realm.len() <= u8::MAX as usize, "realm name too long");
        out.push(self.realm.len() as u8);
        out.extend_from_slice(self.realm.as_bytes());
        out.extend_from_slice(&self.cell.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Option<(CellRef, &[u8])> {
        let (&len, rest) = bytes.split_first()?;
        let (realm, rest) = take(rest, len as usize)?;
        let realm = std::str::from_utf8(realm).ok()?.to_owned();
        let (cell, rest) = u64_le(rest)?;
        Some((CellRef { realm, cell }, rest))
    }
}

/// A message on a cell's gossip topics.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GossipMsg {
    /// One live signed action (the `actions` topic).
    Action(SignedAction),
    /// Periodic state summary for divergence detection (the `state` topic).
    Beacon(StateBeacon),
    /// One signed proposal manifest (the `proposals` topic). Boxed: the
    /// manifest dwarfs every other variant.
    Proposal(Box<SignedProposal>),
    /// One signed ballot (the `proposals` topic).
    Vote(SignedVote),
    /// One signed finality certificate (the `proposals` topic). Boxed: a
    /// full-roster certificate is the largest gossip payload (~131 KiB).
    Certificate(Box<SignedCertificate>),
}

/// Summary of a peer's view of the cell: the per-author sequence vector and
/// the world content hash.
///
/// Two peers whose sequence vectors are equal have applied the same set of
/// actions; if their hashes still differ, application order diverged and one
/// of them must resync. An author whose remote seq is ahead of ours reveals
/// gossip we missed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StateBeacon {
    pub log_len: u64,
    /// `(author, last accepted seq)`, sorted by author bytes (canonical).
    pub seqs: Vec<(PlayerId, u64)>,
    pub content_hash: u64,
}

impl GossipMsg {
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            GossipMsg::Action(signed) => {
                out.push(0);
                signed.encode(out);
            }
            GossipMsg::Beacon(beacon) => {
                out.push(1);
                out.extend_from_slice(&beacon.log_len.to_le_bytes());
                assert!(beacon.seqs.len() <= u16::MAX as usize);
                out.extend_from_slice(&(beacon.seqs.len() as u16).to_le_bytes());
                for (author, seq) in &beacon.seqs {
                    out.extend_from_slice(&author.0);
                    out.extend_from_slice(&seq.to_le_bytes());
                }
                out.extend_from_slice(&beacon.content_hash.to_le_bytes());
            }
            GossipMsg::Proposal(signed) => {
                out.push(2);
                signed.encode(out);
            }
            GossipMsg::Vote(signed) => {
                out.push(3);
                signed.encode(out);
            }
            GossipMsg::Certificate(signed) => {
                out.push(4);
                signed.encode(out);
            }
        }
    }

    /// Decode exactly one gossip message; rejects trailing bytes and, for
    /// beacons, author lists that are not strictly increasing.
    pub fn decode(bytes: &[u8]) -> Option<GossipMsg> {
        let (&tag, rest) = bytes.split_first()?;
        match tag {
            0 => {
                let (signed, rest) = SignedAction::decode(rest)?;
                rest.is_empty().then_some(GossipMsg::Action(signed))
            }
            1 => {
                let (log_len, rest) = u64_le(rest)?;
                let (n_authors, mut rest) = u16_le(rest)?;
                let mut seqs: Vec<(PlayerId, u64)> = Vec::new();
                for _ in 0..n_authors {
                    let (author, tail) = take(rest, 32)?;
                    let author = PlayerId(author.try_into().unwrap());
                    let (seq, tail) = u64_le(tail)?;
                    if let Some(&(last, _)) = seqs.last()
                        && author.0 <= last.0
                    {
                        return None; // non-canonical order or duplicate
                    }
                    seqs.push((author, seq));
                    rest = tail;
                }
                let (content_hash, rest) = u64_le(rest)?;
                rest.is_empty().then_some(GossipMsg::Beacon(StateBeacon {
                    log_len,
                    seqs,
                    content_hash,
                }))
            }
            2 => {
                let (signed, rest) = SignedProposal::decode(rest)?;
                rest.is_empty()
                    .then(|| GossipMsg::Proposal(Box::new(signed)))
            }
            3 => {
                let (signed, rest) = SignedVote::decode(rest)?;
                rest.is_empty().then_some(GossipMsg::Vote(signed))
            }
            4 => {
                let (signed, rest) = SignedCertificate::decode(rest)?;
                rest.is_empty()
                    .then(|| GossipMsg::Certificate(Box::new(signed)))
            }
            _ => None,
        }
    }
}

/// Request on the `/civora/sync/1` request-response protocol.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SyncRequest {
    /// Ask a peer for the cell's current snapshot and signed action log.
    Join {
        proto: u32,
        chunk_size: u32,
        cell: CellRef,
    },
}

impl SyncRequest {
    /// A well-formed join request for the current protocol.
    pub fn join(cell: CellRef) -> Self {
        SyncRequest::Join {
            proto: PROTO_VERSION,
            chunk_size: CHUNK_SIZE as u32,
            cell,
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            SyncRequest::Join {
                proto,
                chunk_size,
                cell,
            } => {
                out.push(0);
                out.extend_from_slice(&proto.to_le_bytes());
                out.extend_from_slice(&chunk_size.to_le_bytes());
                cell.encode(out);
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<SyncRequest> {
        let (&tag, rest) = bytes.split_first()?;
        match tag {
            0 => {
                let (proto, rest) = u32_le(rest)?;
                let (chunk_size, rest) = u32_le(rest)?;
                let (cell, rest) = CellRef::decode(rest)?;
                rest.is_empty().then_some(SyncRequest::Join {
                    proto,
                    chunk_size,
                    cell,
                })
            }
            _ => None,
        }
    }
}

/// Why a join request was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RejectReason {
    ProtoMismatch = 0,
    ChunkSizeMismatch = 1,
    UnknownCell = 2,
    /// The peer is itself still syncing and cannot serve a snapshot.
    NotReady = 3,
}

impl RejectReason {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::ProtoMismatch),
            1 => Some(Self::ChunkSizeMismatch),
            2 => Some(Self::UnknownCell),
            3 => Some(Self::NotReady),
            _ => None,
        }
    }
}

/// Response on the `/civora/sync/1` protocol.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SyncResponse {
    /// The cell's full signed action log (log order) and world snapshot.
    ///
    /// Chunks are sorted by [`ChunkPos`] with fully-empty chunks omitted —
    /// the same canonical order [`VoxelWorld::content_hash`] hashes in — and
    /// each payload is exactly [`CHUNK_BYTES`] in [`Chunk::block_bytes`]
    /// order.
    Accept {
        proto: u32,
        cell: CellRef,
        content_hash: u64,
        log: Vec<SignedAction>,
        chunks: Vec<(ChunkPos, Vec<u8>)>,
        /// Accepted governance state: each proposal with the certificate that
        /// carried it to finality, so a late joiner rebuilds its ledger.
        ledger: Vec<(SignedProposal, SignedCertificate)>,
        /// Proposals whose voting window is still open.
        open_proposals: Vec<SignedProposal>,
        /// Ballots for those open proposals.
        open_votes: Vec<SignedVote>,
    },
    Reject {
        reason: RejectReason,
    },
}

impl SyncResponse {
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            SyncResponse::Accept {
                proto,
                cell,
                content_hash,
                log,
                chunks,
                ledger,
                open_proposals,
                open_votes,
            } => {
                out.push(0);
                out.extend_from_slice(&proto.to_le_bytes());
                cell.encode(out);
                out.extend_from_slice(&content_hash.to_le_bytes());
                out.extend_from_slice(&(log.len() as u32).to_le_bytes());
                for entry in log {
                    entry.encode(out);
                }
                out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
                for (pos, bytes) in chunks {
                    debug_assert_eq!(bytes.len(), CHUNK_BYTES);
                    for coord in [pos.x, pos.y, pos.z] {
                        out.extend_from_slice(&coord.to_le_bytes());
                    }
                    out.extend_from_slice(bytes);
                }
                out.extend_from_slice(&(ledger.len() as u32).to_le_bytes());
                for (proposal, certificate) in ledger {
                    proposal.encode(out);
                    certificate.encode(out);
                }
                out.extend_from_slice(&(open_proposals.len() as u32).to_le_bytes());
                for proposal in open_proposals {
                    proposal.encode(out);
                }
                out.extend_from_slice(&(open_votes.len() as u32).to_le_bytes());
                for vote in open_votes {
                    vote.encode(out);
                }
            }
            SyncResponse::Reject { reason } => {
                out.push(1);
                out.push(*reason as u8);
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<SyncResponse> {
        let (&tag, rest) = bytes.split_first()?;
        match tag {
            0 => {
                let (proto, rest) = u32_le(rest)?;
                let (cell, rest) = CellRef::decode(rest)?;
                let (content_hash, rest) = u64_le(rest)?;
                let (n_log, mut rest) = u32_le(rest)?;
                let mut log = Vec::new();
                for _ in 0..n_log {
                    let (entry, tail) = SignedAction::decode(rest)?;
                    log.push(entry);
                    rest = tail;
                }
                let (n_chunks, mut rest) = u32_le(rest)?;
                let mut chunks: Vec<(ChunkPos, Vec<u8>)> = Vec::new();
                for _ in 0..n_chunks {
                    let (x, tail) = i32_le(rest)?;
                    let (y, tail) = i32_le(tail)?;
                    let (z, tail) = i32_le(tail)?;
                    let pos = ChunkPos::new(x, y, z);
                    if let Some((last, _)) = chunks.last()
                        && pos <= *last
                    {
                        return None; // non-canonical chunk order or duplicate
                    }
                    let (blocks, tail) = take(tail, CHUNK_BYTES)?;
                    if blocks.iter().all(|&b| b == 0) {
                        return None; // empty chunks must be omitted
                    }
                    chunks.push((pos, blocks.to_vec()));
                    rest = tail;
                }
                let (n_ledger, mut rest) = u32_le(rest)?;
                if n_ledger as usize > MAX_SYNC_LEDGER_ENTRIES {
                    return None;
                }
                let mut ledger = Vec::new();
                for _ in 0..n_ledger {
                    let (proposal, tail) = SignedProposal::decode(rest)?;
                    let (certificate, tail) = SignedCertificate::decode(tail)?;
                    ledger.push((proposal, certificate));
                    rest = tail;
                }
                let (n_open, mut rest) = u32_le(rest)?;
                if n_open as usize > MAX_SYNC_OPEN_PROPOSALS {
                    return None;
                }
                let mut open_proposals = Vec::new();
                for _ in 0..n_open {
                    let (proposal, tail) = SignedProposal::decode(rest)?;
                    open_proposals.push(proposal);
                    rest = tail;
                }
                let (n_votes, mut rest) = u32_le(rest)?;
                if n_votes as usize > MAX_SYNC_VOTES {
                    return None;
                }
                let mut open_votes = Vec::new();
                for _ in 0..n_votes {
                    let (vote, tail) = SignedVote::decode(rest)?;
                    open_votes.push(vote);
                    rest = tail;
                }
                rest.is_empty().then_some(SyncResponse::Accept {
                    proto,
                    cell,
                    content_hash,
                    log,
                    chunks,
                    ledger,
                    open_proposals,
                    open_votes,
                })
            }
            1 => match rest {
                [reason] => Some(SyncResponse::Reject {
                    reason: RejectReason::from_byte(*reason)?,
                }),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Extract a world's chunks in canonical snapshot order: sorted by
/// [`ChunkPos`], fully-empty chunks omitted (matching `content_hash`).
pub fn snapshot_chunks(world: &VoxelWorld) -> Vec<(ChunkPos, Vec<u8>)> {
    let mut positions: Vec<ChunkPos> = world.chunk_positions().collect();
    positions.sort();
    positions
        .into_iter()
        .filter_map(|pos| {
            let chunk = world.chunk(pos)?;
            if chunk.is_empty() {
                return None;
            }
            Some((pos, chunk.block_bytes().collect()))
        })
        .collect()
}

/// Rebuild a world from transferred snapshot chunks.
///
/// Returns `None` if any payload is not a valid chunk. The caller must still
/// compare [`VoxelWorld::content_hash`] against the advertised hash before
/// trusting the result.
pub fn world_from_chunks(chunks: &[(ChunkPos, Vec<u8>)]) -> Option<VoxelWorld> {
    let mut world = VoxelWorld::new();
    for (pos, bytes) in chunks {
        world.insert_chunk(*pos, Chunk::from_block_bytes(bytes)?);
    }
    Some(world)
}

#[cfg(test)]
mod tests {
    use super::*;
    use civora_identity::Identity;
    use civora_sim::{Action, BlockId, tick};

    fn identity() -> Identity {
        Identity::from_seed([7; 32])
    }

    fn signed(seq: u64) -> SignedAction {
        identity().sign(
            Action::PlaceBlock {
                pos: [1, 4, seq as i32],
                block: BlockId::PLANK,
            },
            seq,
        )
    }

    #[test]
    fn cell_ref_round_trips_and_names_topics() {
        let cell = CellRef::genesis();
        let mut bytes = Vec::new();
        cell.encode(&mut bytes);
        let (decoded, rest) = CellRef::decode(&bytes).unwrap();
        assert_eq!(decoded, cell);
        assert!(rest.is_empty());
        assert_eq!(cell.actions_topic(), "civora/2/genesis/0/actions");
        assert_eq!(cell.state_topic(), "civora/2/genesis/0/state");
        assert_eq!(cell.proposals_topic(), "civora/2/genesis/0/proposals");
    }

    #[test]
    fn gossip_action_round_trips() {
        let msg = GossipMsg::Action(signed(5));
        let mut bytes = Vec::new();
        msg.encode(&mut bytes);
        assert_eq!(GossipMsg::decode(&bytes), Some(msg));

        for len in 0..bytes.len() {
            assert_eq!(GossipMsg::decode(&bytes[..len]), None);
        }
        bytes.push(0);
        assert_eq!(GossipMsg::decode(&bytes), None);
    }

    #[test]
    fn gossip_beacon_round_trips_and_enforces_author_order() {
        let a = PlayerId([1; 32]);
        let b = PlayerId([2; 32]);
        let beacon = |seqs| {
            GossipMsg::Beacon(StateBeacon {
                log_len: 9,
                seqs,
                content_hash: 0xdead_beef,
            })
        };

        let msg = beacon(vec![(a, 4), (b, 7)]);
        let mut bytes = Vec::new();
        msg.encode(&mut bytes);
        assert_eq!(GossipMsg::decode(&bytes), Some(msg));

        // Unsorted or duplicate authors are non-canonical.
        for bad in [vec![(b, 7), (a, 4)], vec![(a, 4), (a, 5)]] {
            let mut bytes = Vec::new();
            beacon(bad).encode(&mut bytes);
            assert_eq!(GossipMsg::decode(&bytes), None);
        }
    }

    #[test]
    fn gossip_proposal_and_vote_round_trip() {
        use civora_governance::{
            Cid, Proposal, ProposalId, ProposalKind, RollbackPlan, Vote, VoteChoice,
        };

        let identity = identity();
        let proposal = Proposal {
            kind: ProposalKind::AssetPatch,
            author_public_key: identity.player_id(),
            git_commit_hash: [0xAA; 20],
            source_bundle_cid: Cid([1; 32]),
            build_manifest_cid: Cid([2; 32]),
            wasm_module_cids: vec![],
            asset_cids: vec![Cid([3; 32])],
            migration_cids: vec![],
            governance_change: None,
            test_results_cid: Cid([4; 32]),
            activation_epoch: 7,
            rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
        };
        let vote = Vote {
            proposal_id: ProposalId([5; 32]),
            voter: identity.player_id(),
            choice: VoteChoice::Yes,
        };

        for msg in [
            GossipMsg::Proposal(Box::new(SignedProposal::sign(&identity, proposal))),
            GossipMsg::Vote(SignedVote::sign(&identity, vote)),
        ] {
            let mut bytes = Vec::new();
            msg.encode(&mut bytes);
            assert_eq!(GossipMsg::decode(&bytes), Some(msg));

            for len in 0..bytes.len() {
                assert_eq!(GossipMsg::decode(&bytes[..len]), None);
            }
            bytes.push(0);
            assert_eq!(GossipMsg::decode(&bytes), None);
        }
    }

    #[test]
    fn sync_request_round_trips() {
        let req = SyncRequest::join(CellRef::genesis());
        let mut bytes = Vec::new();
        req.encode(&mut bytes);
        assert!(bytes.len() <= MAX_REQUEST_BYTES);
        assert_eq!(SyncRequest::decode(&bytes), Some(req));

        for len in 0..bytes.len() {
            assert_eq!(SyncRequest::decode(&bytes[..len]), None);
        }
        bytes[0] = 0xff;
        assert_eq!(SyncRequest::decode(&bytes), None);
    }

    #[test]
    fn sync_response_round_trips_via_world() {
        let mut world = VoxelWorld::flat(1);
        tick::step(
            &mut world,
            &[Action::PlaceBlock {
                pos: [1, 4, 1],
                block: BlockId::GLASS,
            }],
        );

        let resp = SyncResponse::Accept {
            proto: PROTO_VERSION,
            cell: CellRef::genesis(),
            content_hash: world.content_hash(),
            log: vec![signed(0), signed(3)],
            chunks: snapshot_chunks(&world),
            ledger: vec![],
            open_proposals: vec![],
            open_votes: vec![],
        };
        let mut bytes = Vec::new();
        resp.encode(&mut bytes);
        assert!(bytes.len() <= MAX_RESPONSE_BYTES);

        let decoded = SyncResponse::decode(&bytes).unwrap();
        assert_eq!(decoded, resp);
        let SyncResponse::Accept {
            chunks,
            content_hash,
            ..
        } = decoded
        else {
            unreachable!()
        };
        // The decoded snapshot reproduces the exact world hash.
        let rebuilt = world_from_chunks(&chunks).unwrap();
        assert_eq!(rebuilt.content_hash(), content_hash);
    }

    #[test]
    fn sync_response_rejects_malformed_input() {
        let world = VoxelWorld::flat(0);
        let resp = SyncResponse::Accept {
            proto: PROTO_VERSION,
            cell: CellRef::genesis(),
            content_hash: world.content_hash(),
            log: vec![signed(0)],
            chunks: snapshot_chunks(&world),
            ledger: vec![],
            open_proposals: vec![],
            open_votes: vec![],
        };
        let mut bytes = Vec::new();
        resp.encode(&mut bytes);

        for len in 0..bytes.len() {
            assert_eq!(SyncResponse::decode(&bytes[..len]), None);
        }
        bytes.push(0);
        assert_eq!(SyncResponse::decode(&bytes), None);

        // Reject round-trip and unknown reason byte.
        let mut bytes = Vec::new();
        SyncResponse::Reject {
            reason: RejectReason::NotReady,
        }
        .encode(&mut bytes);
        assert_eq!(
            SyncResponse::decode(&bytes),
            Some(SyncResponse::Reject {
                reason: RejectReason::NotReady
            })
        );
        bytes[1] = 0xff;
        assert_eq!(SyncResponse::decode(&bytes), None);
    }

    #[test]
    fn snapshot_chunks_are_sorted_and_skip_empty() {
        let mut world = VoxelWorld::new();
        world.set_block([100, 0, 0], BlockId::STONE);
        world.set_block([-100, 0, 0], BlockId::STONE);
        // Allocate a chunk then leave it all air.
        world.set_block([0, 200, 0], BlockId::STONE);
        world.set_block([0, 200, 0], BlockId::AIR);

        let chunks = snapshot_chunks(&world);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].0 < chunks[1].0);
        assert_eq!(
            world_from_chunks(&chunks).unwrap().content_hash(),
            world.content_hash()
        );
    }

    /// A consistent proposal, its accepting certificate, and one yes ballot,
    /// all from the fixed test identity.
    fn gov_fixture() -> (SignedProposal, SignedCertificate, SignedVote) {
        use civora_governance::{Cid, Proposal, ProposalKind, RollbackPlan, Vote, VoteChoice};
        use std::collections::BTreeMap;

        let id = identity();
        let proposal = Proposal {
            kind: ProposalKind::AssetPatch,
            author_public_key: id.player_id(),
            git_commit_hash: [0x33; 20],
            source_bundle_cid: Cid([1; 32]),
            build_manifest_cid: Cid([2; 32]),
            wasm_module_cids: vec![],
            asset_cids: vec![Cid([3; 32])],
            migration_cids: vec![],
            governance_change: None,
            test_results_cid: Cid([4; 32]),
            activation_epoch: 5,
            rollback_plan: RollbackPlan::RevertToLastSignedSnapshot,
        };
        let signed_proposal = SignedProposal::sign(&id, proposal.clone());
        let signed_vote = SignedVote::sign(
            &id,
            Vote {
                proposal_id: proposal.id(),
                voter: id.player_id(),
                choice: VoteChoice::Yes,
            },
        );
        let roster = vec![id.player_id()];
        let ballots: BTreeMap<PlayerId, SignedVote> = [(id.player_id(), signed_vote)].into();
        let certificate =
            SignedCertificate::certify(&id, &proposal, &roster, &ballots, 1, 5).unwrap();
        (signed_proposal, certificate, signed_vote)
    }

    #[test]
    fn gossip_certificate_round_trips() {
        let (_, certificate, _) = gov_fixture();
        let msg = GossipMsg::Certificate(Box::new(certificate));
        let mut bytes = Vec::new();
        msg.encode(&mut bytes);
        assert_eq!(bytes[0], 4, "certificate gossip tag");
        assert_eq!(GossipMsg::decode(&bytes), Some(msg));

        for len in 0..bytes.len() {
            assert_eq!(GossipMsg::decode(&bytes[..len]), None);
        }
        bytes.push(0);
        assert_eq!(GossipMsg::decode(&bytes), None);
    }

    #[test]
    fn sync_response_carries_governance_payload() {
        let world = VoxelWorld::flat(1);
        let (proposal, certificate, vote) = gov_fixture();
        let resp = SyncResponse::Accept {
            proto: PROTO_VERSION,
            cell: CellRef::genesis(),
            content_hash: world.content_hash(),
            log: vec![signed(0)],
            chunks: snapshot_chunks(&world),
            ledger: vec![(proposal.clone(), certificate)],
            open_proposals: vec![proposal],
            open_votes: vec![vote],
        };
        let mut bytes = Vec::new();
        resp.encode(&mut bytes);
        assert_eq!(SyncResponse::decode(&bytes), Some(resp));

        for len in 0..bytes.len() {
            assert_eq!(SyncResponse::decode(&bytes[..len]), None);
        }
        bytes.push(0);
        assert_eq!(SyncResponse::decode(&bytes), None);
    }

    #[test]
    fn sync_response_rejects_over_cap_governance_counts() {
        // An empty-world Accept ends with three zero count fields
        // (ledger, open_proposals, open_votes) — the last 12 bytes.
        let world = VoxelWorld::flat(0);
        let base = SyncResponse::Accept {
            proto: PROTO_VERSION,
            cell: CellRef::genesis(),
            content_hash: world.content_hash(),
            log: vec![],
            chunks: snapshot_chunks(&world),
            ledger: vec![],
            open_proposals: vec![],
            open_votes: vec![],
        };
        let mut encoded = Vec::new();
        base.encode(&mut encoded);
        let n = encoded.len();

        // Each count sits at a known offset from the end; a value over its cap
        // must be rejected outright.
        for (from_end, over) in [
            (12, MAX_SYNC_LEDGER_ENTRIES as u32 + 1),
            (8, MAX_SYNC_OPEN_PROPOSALS as u32 + 1),
            (4, MAX_SYNC_VOTES as u32 + 1),
        ] {
            let mut bytes = encoded.clone();
            let at = n - from_end;
            bytes[at..at + 4].copy_from_slice(&over.to_le_bytes());
            assert_eq!(SyncResponse::decode(&bytes), None, "cap at -{from_end}");
        }
    }
}
