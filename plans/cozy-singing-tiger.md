# Milestone 3: P2P lobby and world cell sync

## Context

PLAN.md build-order item 3 (first unchecked). The M2 seam was built for this: every world mutation is already a `SignedAction` verified into an `ActionLog` before `tick::step` applies it, and Ed25519 was chosen so `PlayerId` doubles as a libp2p peer identity. README's sync table mandates "signed action log + periodic snapshots" for voxel edits. There is no networking code, async runtime, or lobby UI today.

**Approved scope:** rust-libp2p (tokio); gossipsub for live actions, request-response for snapshot sync, mDNS LAN discovery + manual dial. **One cell = whole world** (cell/realm IDs baked into topics/messages so partitioning slots in later; no committees). Lobby = CLI flags (`--host`, `--join [multiaddr]`) + peer roster in the existing HUD overlay.

**Deliverable:** two+ clients share one live voxel world. Joiner gets snapshot + full signed log, verifies both (log via verify-on-append; snapshot via `content_hash`), then exchanges live signed actions over gossipsub. Periodic state beacons detect divergence and trigger resync. No flags = offline, behavior unchanged.

## Architecture

### New crate `crates/civora-net` (no Bevy; civora-sim stays zero-dep)

- Deps: `civora-sim`, `civora-identity`, `libp2p 0.56` (features: tokio, tcp, noise, yamux, gossipsub, mdns, request-response, macros, ed25519 — verify current release at implementation time), `tokio` (rt, macros, sync, time), `futures`. TCP+Noise+Yamux, not QUIC (simpler, no quinn; QUIC addable later without protocol changes).
- `civora_net::spawn(NetConfig) -> NetHandle` starts a dedicated `std::thread` with a current-thread tokio runtime running the swarm loop. Also expose `pub async fn run(config, cmd_rx, evt_tx)` for threadless integration tests.
- Boundary = two channels in `NetHandle`: client→net `tokio::sync::mpsc::UnboundedSender<NetCommand>` (sync, non-blocking send); net→client `std::sync::mpsc::Receiver<NetEvent>` (Bevy drains via `try_iter`; stored in a `Mutex` in a resource). Only `SignedAction`, `PlayerId`, `VoxelWorld`, `ActionLog`, snapshot byte payloads, and `String` addrs cross; libp2p types never reach the client.
- Bevy world remains sole owner of `SimWorld`/`SessionLog` — no shared locks. Snapshot serving round-trips: `NetEvent::SnapshotRequested{request_id}` → client encodes next FixedUpdate → `NetCommand::ProvideSnapshot` → net loop answers the stored ResponseChannel.

### Identity/keys

- libp2p keypair via `Keypair::ed25519_from_bytes(seed)` from the same 32-byte seed, so PeerId ↔ PlayerId are the same key. Add public `Identity::seed_bytes()` (currently `pub(crate) seed()` at `crates/civora-identity/src/identity.rs:52`); doc it as secret material. Keyfile format untouched.
- PeerId → PlayerId without the identify protocol: Ed25519 pubkeys are inlined in the PeerId multihash — decode protobuf pubkey from the digest, `try_into_ed25519()` → 32 bytes. Disconnect peers whose PeerId isn't an inlined Ed25519 key. Unit test: seed → Keypair → PeerId → PlayerId == `Identity::player_id()`.

### Wire format (hand-rolled canonical codecs, no serde; every decoder rejects unknown tags, truncation, trailing bytes — mirroring `Action::decode`)

In **civora-identity** (`signed.rs`), self-delimiting so lists decode by iteration:
```
SignedAction::encode/decode:
  author(32) || seq(u64 LE) || action_len(u16 LE) || action bytes || signature(64)
```
Plus `ActionLog::last_seq(PlayerId) -> Option<u64>` (log.rs) — seeds joiner's `next_seq` and builds beacons.

In **civora-sim** (canonical inverses of existing hash primitives, still zero-dep): `Chunk::from_block_bytes(&[u8]) -> Option<Chunk>` (validates len 32768, recomputes solid_count), `VoxelWorld::insert_chunk(ChunkPos, Chunk)`.

In **civora-net/src/wire.rs**:
- `CellRef { realm: "genesis", cell: u64 = 0 }` placeholder in topics + sync messages — the future-partitioning seam.
- Gossip topics: `civora/1/genesis/0/actions`, `civora/1/genesis/0/state`.
- Gossip msgs (tag byte): `0x00 ActionMsg{signed_action}`; `0x01 StateBeacon{ log_len, n_authors, [author(32)||last_seq]* sorted by author, content_hash(u64 LE) }`.
- Request-response protocol `/civora/sync/1`, u32-LE length framing (req cap 4 KiB, resp cap 64 MiB):
  - `SyncRequest = Join{ proto=1, chunk_size=32, cell_ref }`
  - `SyncResponse = Accept{ proto, cell_ref, content_hash, n_log || SignedAction* (log order), n_chunks || [pos 3×i32 LE || 32768 block bytes]* (sorted by ChunkPos, block_bytes() order, empty chunks omitted — matches content_hash) }` or `Reject{reason u8}`.
  - flat(3) ≈ 98 chunks ≈ 3.1 MiB — fine over TCP.

### Sync protocol

- **Host** (`--host`): listen `/ip4/0.0.0.0/tcp/0`, print multiaddrs (with `/p2p/<peerid>`), subscribe topics, run mDNS, serve Join via the snapshot round-trip. World = `flat(3)` as today.
- **Joiner** (`--join [addr]`): start with empty world, input gated (state `Joining`); dial given addr or first mDNS peer; send Join. On Accept: rebuild `ActionLog` by `append`ing each entry (re-verifies sigs + per-author seq, seeds `last_seq` for all authors); install chunks into fresh world; assert `content_hash` matches advertised (abort join on mismatch); emit `NetEvent::WorldSync{world, log, hash}`. Client swaps `SimWorld`/`SessionLog`, sets `next_seq = log.last_seq(me).map_or(0, |s| s+1)` (rejoin-safe), marks all chunks dirty, unfreezes player, goes `Live`. Gossip received during sync is buffered in the net loop and flushed after; seq check drops entries already in the snapshot.
- **Live**: `drain_action_queue` unchanged (sign → append → apply) plus, on success, `NetCommand::PublishAction(signed)`. Remote `ActionMsg` → `NetEvent::RemoteAction` → `RemoteActionQueue` resource drained on FixedUpdate *before* local actions: `SessionLog::append` (existing kernel gate does sig + seq) then `tick::step`. Remote `SeqReplay` = expected gossipsub redelivery, log at debug. Own echoed messages dropped by author check.
- **Consistency (honest limits)**: no tick counter, no sequencer — concurrent conflicting edits of the same voxel can diverge per arrival order (Place needs air, Break needs solid). Accepted for M3: **eventual consistency + detection + resync**, not host-sequencing (a central sequencer is throwaway architecture the cell-committee milestone replaces properly). Every 100 ticks (5 s) publish `StateBeacon`. Beacon with equal seq-vector but different hash = true divergence → warn + HUD flag; peer with lexicographically larger PlayerId yields and re-runs the join flow against the disagreeing peer (`NetCommand::Resync`). Beacon showing an author seq ahead of ours = missed gossip (gaps are legal in `ActionLog` but the sim missed an action) → same resync path. Duplicates die on seq check; no hold-back buffer.

### Client integration

- New `crates/civora-client/src/cli.rs`: hand-rolled parse of `--host`, `--join [multiaddr]`, `--key-file <path>` (+ `CIVORA_KEY_FILE` env). Key-path override needed so two instances on one machine have distinct identities — wire into `identity.rs::key_path()`. No flags → `NetMode::Offline`: NetPlugin inserts inert resources, adds no systems; single-player bit-identical.
- New `crates/civora-client/src/net.rs` (`NetPlugin`): resources `NetChannels`, `PeerRoster(Vec<(PlayerId, String)>)`, `NetStatus{mode, listen_addrs, last_divergence}`, `RemoteActionQueue`. FixedUpdate systems ordered before `drain_action_queue`: `pump_net_events` (roster, remote actions, WorldSync swap, SnapshotRequested → ProvideSnapshot), `apply_remote_actions`, `publish_beacon`.
- `sim_bridge.rs`: publish hook after successful append; gate off while `Joining`. `interact.rs`: suppress input during `Joining`; freeze player until WorldSync.
- `main.rs`: parse CLI before identity load; `civora_net::spawn(...)` after identity load, before window opens (listen addr visible on terminal).
- `hud.rs`: `net <mode> — N peers`, per-peer `peer <short-id> <addr>` lines, `DIVERGED — resyncing` flag.

## Implementation order

1. civora-identity: `SignedAction::encode/decode`, `ActionLog::last_seq`, public `Identity::seed_bytes()`, re-exports + tests.
2. civora-sim: `Chunk::from_block_bytes`, `VoxelWorld::insert_chunk` + tests.
3. New `crates/civora-net` (workspace member): `wire.rs` + unit tests.
4. `civora-net/src/peer.rs`: keypair derivation, PeerId↔PlayerId + test.
5. `civora-net`: `codec.rs` (req-resp framing), `behaviour.rs` (gossipsub+mdns+request_response), `event_loop.rs` (swarm loop, join/serve state machines, beacons), `lib.rs` (NetConfig/NetCommand/NetEvent/NetHandle, spawn, async run). Pin libp2p version and confirm builder APIs here.
6. `civora-net/tests/sync.rs`: two in-process nodes, direct dial 127.0.0.1 (no mDNS → CI-safe): join+snapshot hash match, live gossip convergence, duplicate rejected as SeqReplay. Also simulate divergence → resync.
7. Client: `cli.rs`, key-path override, `main.rs` wiring.
8. Client: `net.rs` plugin/resources/systems; `sim_bridge.rs` hook + gating; `interact.rs` gating.
9. Client: beacons, divergence handling/resync, `hud.rs` lines.
10. `cargo test --workspace`, clippy `-D warnings`, fmt; manual verification; update PLAN.md per house convention (check off item 3, Status entry, Build notes: flags, `CIVORA_KEY_FILE`, two-instance recipe, divergence semantics, mDNS caveats); commit `Cargo.lock`.

## Verification

- Unit/integration tests above run in CI unchanged (`ci.yml` already covers the workspace; libp2p tcp stack is pure Rust — no new system libs; build time +1–2 min, Cargo.lock grows ~150 crates).
- Manual two-instance recipe (goes into PLAN.md Build notes):
  ```
  # terminal 1
  CIVORA_PASSPHRASE=a cargo run -p civora-client -- --host --key-file /tmp/civ-a.key
  # prints: listening on /ip4/…/tcp/PORT/p2p/PEERID
  # terminal 2
  CIVORA_PASSPHRASE=b cargo run -p civora-client -- --join /ip4/127.0.0.1/tcp/PORT/p2p/PEERID --key-file /tmp/civ-b.key
  ```
  Verify: HUDs show each other's peer; blocks placed on one appear on the other; F12/`CIVORA_SCREENSHOT` for scripted evidence; `--join` with no addr exercises mDNS.

## Risks

- libp2p 0.5x API churn — pin version, confirm `SwarmBuilder`/`mdns::tokio::Behaviour`/`request_response::Behaviour::with_codec` signatures at step 5.
- Divergence under concurrent conflicting edits is real and accepted; beacon+resync is the mitigation and is covered by the integration test.
- Gossip loss → silent seq gap until next beacon (≤5 s) — acceptable, documented.
- Same key file in two instances breaks per-author seq — unsupported; `--key-file` exists for this.
- mDNS flaky on VPN/filtered Wi-Fi — manual dial is the guaranteed path.
- Whole-world snapshots don't scale — fine at flat(3); `CellRef` in Join is the partitioning seam for the next networking milestone.
