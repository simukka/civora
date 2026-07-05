![image](assets/logo/logo_1024.png)

Build it as a game first, but architect it like a universe protocol. 
The first playable thing should be small and concrete: a peer-to-peer 
voxel survival/building world with portals, AI-assisted mod creation, 
and live community voting. The “universe” emerges when every world, 
rule set, asset pack, economy, and governance system is just another 
voted-in module.

The hard boundary: no authoritative game server, not “no infrastructure 
at all.” We will still need peer discovery, relays, content storage, 
and bootstrapping, but those should be community-run and replaceable, 
not owned by one company.

The key rule is:
* Git commits do not directly change reality.
* Git commits become proposals.
* Proposals become reality only after signed player approval.

That keeps “anyone can push” alive without letting one malicious patch 
brick everyone’s client.

### Recommended stack
Use Rust as the primary language. The project needs a safe native client, 
a deterministic simulation core, peer-to-peer networking, sandboxed plugins, 
cryptographic signatures, and reproducible build tooling. Use Bevy for the 
initial engine because it is a Rust-native, open-source ECS game engine 
with cross-platform support and asset hot-reloading features. Bevy’s ECS 
model also maps well to “world rules as systems.”

Use WebAssembly for user-submitted gameplay code. Do not hot-patch arbitrary 
native code. Every community rule, item behavior, NPC behavior, portal rule, 
governance extension, or AI-generated mechanic should compile into a sandboxed 
Wasm module. WebAssembly’s security model is specifically built around isolating 
modules from the host runtime, and Wasmtime gives us a production-grade 
runtime for Wasm, WASI, and the Component Model.

Use WIT / WebAssembly Component Model for the plugin ABI. This lets us define 
stable interfaces like spawn_entity, read_voxel, emit_event, mint_item, cast_vote, 
and open_portal, while allowing modules to be written in Rust first and other 
languages later. The Component Model is designed for interoperable WebAssembly 
libraries, applications, and environments.

Use libp2p for networking. It is a modular peer-to-peer networking stack with 
transports such as TCP, QUIC, WebSocket, WebRTC, and WebTransport, which is 
exactly the kind of base layer we need for a no-authoritative-server 
multiplayer network.

Use content-addressed storage for patches, assets, and world snapshots. 
Every accepted commit should point to immutable content hashes. IPFS-style 
CIDs and Merkle DAGs are a good conceptual model: data is addressed by 
hash-derived identifiers rather than by mutable server locations.

Use CRDTs only where they fit: collaborative documents, build plans, 
low-stakes world editing, chat, map annotations, and social metadata. 
Do not use CRDTs as the whole real-time combat/physics system. Automerge 
is useful here because it is a local-first sync engine intended for 
multiplayer apps that work offline and prevent conflicts.

Use Bazel or Nix for reproducible builds. The system depends on different 
peers building the same source and getting the same artifact hashes. 
Bazel’s hermetic build model is relevant because it isolates builds 
from host-machine differences and pins toolchains/dependencies.

| Area                            | Language / tech                                                      |
| ------------------------------- | -------------------------------------------------------------------- |
| Engine kernel                   | Rust                                                                 |
| Game engine                     | Bevy / Rust                                                          |
| P2P networking                  | Rust + libp2p                                                        |
| User-submitted gameplay modules | Rust compiled to WebAssembly first                                   |
| Plugin ABI                      | WIT / WebAssembly Component Model                                    |
| Wasm runtime                    | Wasmtime                                                             |
| Shaders                         | WGSL                                                                 |
| local assistant UI              | Rust, but keep it outside the deterministic simulation               |
| Build system                    | Bazel or Nix                                                         |
| Asset format                    | glTF, PNG, KTX2, Ogg/Opus, voxel chunks                              |
| Source workflow                 | Git commits + signed proposal manifests                              |

### Layers
```
┌──────────────────────────────────────────────┐
│  Player Client                               │
│                                              │
│  Bevy Renderer / Input / Audio / UI          │
│  Voxel Realm / Portal Realm / Future Realms  │
├──────────────────────────────────────────────┤
│  Mutable Universe Layer                      │
│  - Wasm gameplay modules                     │
│  - Realm rules                               │
│  - Item definitions                          │
│  - Governance module                         │
│  - AI-generated content proposals            │
├──────────────────────────────────────────────┤
│  Reality Kernel                              │
│  - Wasm sandbox                              │
│  - Capability permissions                    │
│  - Deterministic scheduler                   │
│  - Patch loader / rollback                   │
│  - Signature verification                    │
├──────────────────────────────────────────────┤
│  P2P Protocol Layer                          │
│  - libp2p gossip                             │
│  - peer discovery                            │
│  - DHT / content lookup                      │
│  - vote broadcast                            │
│  - cell committees                           │
├──────────────────────────────────────────────┤
│  Local Data Layer                            │
│  - accepted proposal ledger                  │
│  - content-addressed assets                  │
│  - world snapshots                           │
│  - local player keys                         │
└──────────────────────────────────────────────┘
```

### Kernel
The Reality Kernel is the only thing we would not make freely hot-patchable at first. 
It should be tiny, audited, boring, and hard to change. Everything above it can be 
modified by vote: game rules, assets, portals, economics, crafting, AI tools, 
even governance. But the kernel must always verify signatures, enforce sandboxing, 
protect player keys, stop infinite loops, and allow rollback.

That is the difference between a self-evolving universe and a remote-code-execution disaster.

## Open contribution
Git as the authoring interface.

A player or AI agent creates a branch:
```
feature/add-floating-islands
```
Then the client packages the commit into a proposal manifest:
```
Proposal {
  proposal_id
  author_public_key
  git_commit_hash
  source_bundle_cid
  build_manifest_cid
  wasm_module_cids
  asset_cids
  migration_cids
  governance_change: optional
  test_results_cid
  activation_epoch
  rollback_plan
}
```
The proposal is broadcast over the P2P network. Other clients fetch the source and 
artifacts by content hash, verify the build, run local tests, show the diff to players, 
and ask for a vote.

A commit becomes real only when the current governance rule produces a valid finality 
certificate:
```
FinalityCertificate {
  proposal_id
  governance_rule_version
  eligible_roster_root
  yes_signatures
  no_signatures
  quorum_result
  accepted_epoch
}
```
Every client follows the accepted ledger. If your client sees a valid finality certificate, 
it downloads the content pack, verifies it, and loads it at the next safe patch boundary.

## Voting
A proposal passes if more than 50% of currently eligible online players vote yes during 
the voting window, with a minimum quorum.

For the first alpha, “eligible” should mean:

1. The player has a valid identity key.
2. The player has joined the world before the proposal opened.
3. The player is online or recently active during the vote epoch.
4. The player is not a newly created untrusted identity.

Do not start with pure token-weighted voting. That turns the universe into plutocracy 
before you have culture, identity, anti-Sybil protection, or social legitimacy.

Use tokens first as AI / compute / proposal credits, not as the main political power.

### Initial voting rules
| Change type                 | Requirement                       |
| --------------------------- | --------------------------------- |
| Asset-only patch            | Simple majority                   |
| New item / creature / biome | Simple majority + automated tests |
| Gameplay code patch         | Majority + sandbox validation     |
| Economy change              | Higher quorum                     |
| Governance change           | Higher quorum + activation delay  |
| Kernel change               | Not in-game hot patch for v1      |

The governance module itself can be changed by vote, but the first governance rule should 
require an activation delay. That gives players time to inspect, fork, or reject a 
hostile rule change.

## Hot patching
Hot patch at epoch boundaries, not instantly in the middle of simulation.
```
12:00:00 - Proposal accepted
12:00:10 - Clients fetch patch
12:00:20 - Clients verify signatures and content hashes
12:00:30 - Clients run migration dry-run
12:01:00 - Patch activates at epoch 184220
12:01:01 - New rules live
```

| Patch type      | Example                                             | Hot-patch difficulty        |
| --------------- | --------------------------------------------------- | --------------------------- |
| Asset patch     | New texture, mesh, sound, biome config              | Easy                        |
| Data patch      | New item stats, recipes, spawn tables               | Medium                      |
| Wasm rule patch | New physics behavior, AI NPC logic, governance rule | Hard but doable             |
| Kernel patch    | Networking, sandbox, signature verifier             | Do not auto-hot-patch in v1 |


Every patch needs a rollback plan. If a migration corrupts a realm, 
clients should return to the last signed snapshot.

## Multiplayer 
We cannot make one global real-time simulation where every player validates every tick. 
That will collapse. Use realm + cell partitioning. 

### A realm is a rule universe.
Each realm has: 
```
RealmManifest {
  realm_id
  rule_modules
  physics_profile
  asset_pack
  portal_interfaces
  economy_rules
  governance_scope
}
```

### Cell
A cell is a local simulation area inside a realm.

In one realm, a cell might be a chunk group. 
In another realm, it might be a star system. 
In some realms, it might be a match.

Each cell is maintained by a temporary peer committee:
```
CellCommittee {
  cell_id
  members
  current_tick
  signed_snapshot_hash
  action_log_hash
}
```

The committee validates actions, gossips signed state updates, and periodically publishes 
snapshots. If peers leave, the committee rotates.

This is “no central server,” but not “no authority.” Authority is temporary, local, 
distributed, auditable, and replaceable.

## World-state
Use different synchronization models for different kinds of state.
This avoids forcing one distributed-systems model to solve every problem:
| State type                 | Model                                        |
| -------------------------- | -------------------------------------------- |
| Player movement            | Prediction + rollback / reconciliation       |
| Combat                     | Deterministic tick simulation inside a cell  |
| Voxel edits                | Signed action log + periodic snapshots       |
| Large collaborative builds | CRDT-assisted editing                        |
| Economy transactions       | Ledger transactions                          |
| Governance votes           | Signed append-only ledger                    |
| Portals                    | Cross-realm transaction receipts             |
| Chat / notes / map labels  | CRDT/local-first sync                        |
| AI proposals               | Git commit + content-addressed artifact pack |

## Portals
A portal is not just a door. It is a protocol bridge.
Each realm exposes an avatar import/export interface:
```
export_avatar(player_id) -> AvatarState
import_avatar(AvatarState, PortalTicket) -> SpawnResult
```

A portal transaction says:
```
Player A leaves Genesis Voxel Realm at portal P.
Inventory subset X is locked or transformed.
Avatar state is exported.
Destination realm accepts compatible state.
Player appears in Space Realm.
```

Not every object should transfer. A diamond pickaxe may become raw mass, trade credit, 
or nothing in a space realm. Each portal defines conversion rules.

This lets you connect Minecraft-like, EVE-like, and bizarre AI-generated realms without 
pretending they all share identical physics.

## Making reality
Give every player an in-game tool called something like "Reality Forge".
A player prompts their AI agent, it produces:
```
- biome config
- voxel palette
- item definitions
- optional Wasm rule module
- tests
- preview realm
- proposal summary
```

The player tests it privately. Then they submit it as a signed proposal.

| AI use                   | Example                              |
| ------------------------ | ------------------------------------ |
| Generate assets          | Textures, models, sounds             |
| Generate code            | Wasm module drafts                   |
| Run simulations          | Balance testing                      |
| Spawn private test realm | Preview before proposing             |
| Submit proposal          | Anti-spam deposit                    |

### Reputation
Non-transferable. Used for trust, moderation weight, anti-spam, and committee selection.
Do not let reputation be sold. 
The universe needs identity and trust more than it needs speculation.

# First milestone: 90-day vertical slice
The first 90 days should prove the thesis, not the whole universe. 
The win is not player count yet. The win is proving that reality can 
be modified socially, safely, and live.

## Target demo
Twelve to thirty-two players join a peer-to-peer voxel world.
One player uses AI to generate a new object.
All online players receive an in-game vote.
The majority approves.
Every client fetches the content-addressed patch, verifies signatures, 
loads the Wasm module, and the new object appears in the live world 
without restarting.

That is the magic moment.

## 90-day deliverables

| Area         | Deliverable                              |
| ------------ | ---------------------------------------- |
| Client       | Native Rust/Bevy app                     |
| World        | Small voxel map with building/mining     |
| Network      | P2P join, discovery, gossip              |
| Identity     | Player keypair and signed actions        |
| Governance   | Proposal, vote, finality certificate     |
| Patch system | Asset + Wasm hot patch at epoch boundary |
| AI           | Prompt-to-proposal prototype             |
| Portal       | One portal to a tiny second test realm   |
| Build        | Reproducible proposal pack               |
| Security     | Wasm permissions, fuel limits, rollback  |

## Six-month target
| System     | Goal                                                    |
| ---------- | ------------------------------------------------------- |
| Players    | 100–300 concurrent across multiple cells                |
| Realms     | Genesis Voxel Realm + one experimental portal realm     |
| Governance | Changeable voting rules with activation delay           |
| AI         | Prompt-to-item, prompt-to-biome, prompt-to-NPC behavior |
| Patches    | Live assets, data, and Wasm gameplay modules            |
| Economy    | Internal Reality Tokens and reputation                  |
| Storage    | Content-addressed asset packs pinned by peers           |
| Moderation | Proposal review, warnings, blocklists, local forks      |
| Forking    | Players can follow alternate accepted ledgers           |

## What to build first

Build in this exact order:

- [x] Rust/Bevy voxel client *(done 2026-07-05)*
- [x] Player identity and signed actions *(done 2026-07-05)*
- [x] P2P lobby and world cell sync (plans/cozy-singing-tiger.md) *(done 2026-07-05)*
- [ ] Proposal manifest format
- [ ] Voting UI
- [ ] Accepted proposal ledger
- [ ] Content-addressed patch packs
- [ ] Wasm plugin ABI
- [ ] Asset hot patch
- [ ] Gameplay Wasm hot patch
- [ ] Portal to second realm
- [ ] Governance-rule patching
- [ ] Reputation economy

## Status

### Milestone 1: voxel client (done 2026-07-05)

Cargo workspace with two crates:

- `crates/civora-sim` — deterministic simulation core with **zero
  dependencies** (no Bevy, no I/O). Chunks (32³), sparse voxel world,
  action queue semantics (`tick::step`), DDA raycast, FNV content hash.
  This is the seed of the Reality Kernel's deterministic scheduler: all
  world mutation flows through `Action`s so signed action logs and cell
  validation can wrap it later without a rewrite.
- `crates/civora-client` — Bevy 0.19 client. Culled-face chunk meshing
  with vertex colors, flat test world, first-person controller with
  voxel AABB collision (walk/jump/fly), break/place through the action
  queue, crosshair + hotbar + debug overlay.

CI (`.github/workflows/ci.yml`): fmt + clippy + tests on Linux, then
release client builds for Linux, Windows, and macOS uploaded as
artifacts.

### Milestone 2: player identity and signed actions (done 2026-07-05)

New crate `crates/civora-identity` — Ed25519 keypairs (`ed25519-dalek`,
the same scheme libp2p peer IDs use, so the P2P milestone can reuse the
key):

- `SignedAction` binds an action to its author and a per-author sequence
  number via a domain-separated signature (`civora.action.v1`) over the
  canonical `Action::encode` bytes (new in `civora-sim`, still
  dependency-free).
- `ActionLog` is append-only and verify-on-append: bad signatures and
  non-increasing sequence numbers (replays) never enter it.
  `verify_and_replay` re-checks the whole log and replays it onto a fresh
  world; tests prove the result matches the directly-mutated world's
  `content_hash` — the audit path cell committees will use later.
- The key is stored passphrase-encrypted (Argon2id KDF +
  XChaCha20-Poly1305), owner-only file permissions on Unix.

Client: unlocks or creates the key file on the terminal before the window
opens; every break/place is signed and must verify into the session log
before `tick::step` applies it (the Reality Kernel gate); the debug HUD
shows the player id and signed-action count.

### Milestone 3: P2P lobby and world cell sync (done 2026-07-05)

New crate `crates/civora-net` — libp2p 0.56 (TCP + Noise + Yamux) on a
dedicated tokio thread; the Bevy client talks to it over plain channels
and libp2p types never cross that boundary. Implements the "signed
action log + periodic snapshots" model for voxel edits with **one cell =
the whole world** (`genesis/0`); the `CellRef` in every topic name and
sync message is the seam where cell partitioning slots in later.

- **Lobby**: the player's Ed25519 identity key is also the libp2p
  transport key, so the connection-authenticated `PeerId` *is* the
  `PlayerId` that signs actions — no extra binding handshake. Peers whose
  id is not an inlined Ed25519 key are disconnected. Discovery is mDNS
  (LAN) plus direct dial.
- **Join** (`/civora/sync/1` request-response): the joiner receives the
  full signed action log and a world snapshot (canonical chunk order,
  matching `content_hash`). The log is rebuilt through
  `ActionLog::append` — re-verifying every signature and per-author
  seq — and the snapshot must reproduce the advertised content hash, or
  the join is refused. Rejoining with the same identity resumes its seq
  numbering from the transferred log.
- **Live sync**: locally signed actions that pass the kernel gate are
  gossiped (gossipsub); remote actions enter through the same gate
  (`SessionLog::append`, then `tick::step`). Gossip redelivery dies on
  the seq check.
- **Divergence detection**: every 5 s each peer publishes a state beacon
  (per-author seq vector + content hash). Equal seq vectors with
  different hashes = true divergence (concurrent conflicting edits are
  possible — there is no sequencer by design; committees arrive in a
  later milestone): the peer with the lexicographically larger PlayerId
  yields and resyncs. A seq deficit that persists across two beacons =
  missed gossip, same recovery. The HUD shows `DIVERGED - resyncing`.
- **Client**: `--host` / `--join [multiaddr]` / `--key-file` flags
  (offline single player is unchanged when no flags are given); joiners
  start with an empty world and gated input until the sync lands; the
  debug HUD shows net phase and the peer roster.
- Integration test (`crates/civora-net/tests/sync.rs`): two real swarm
  nodes over loopback TCP — join + hash match, bidirectional gossip
  convergence, replay rejection, resync recovery. mDNS is off in tests
  (CI runners filter multicast).

## Build notes

Things to know that are not obvious from the code:

- **Rust toolchain**: Bevy 0.19 requires rustc ≥ 1.95. Update with
  `rustup update stable`.
- **Linux system packages**: building needs `libasound2-dev` (audio).
  Gamepad support (`bevy_gilrs`) is currently excluded from the Bevy
  feature list because it needs `libudev-dev`; install that package and
  add the feature to `crates/civora-client/Cargo.toml` to enable it.
  CI installs `libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev`.
- **Bevy features**: the client pins an explicit Bevy feature list
  (expansion of the `3d` + `ui` + `audio` umbrellas minus `bevy_gilrs`);
  see the comment in `crates/civora-client/Cargo.toml`.
- **Cargo.lock is committed** so CI and every peer build the same
  dependency versions — this matters for the reproducible-build goal.
- **macOS artifact is arm64** (`macos-latest` runner is Apple Silicon).
  Add an `x86_64-apple-darwin` matrix entry if Intel Macs are needed.
- **Identity key file**: `<OS config dir>/civora/identity.key` (Linux:
  `~/.config/civora/identity.key`). The client prompts for the passphrase
  on the terminal at startup; set `CIVORA_PASSPHRASE` to skip the prompt
  (scripted runs, launches without a terminal). There is no passphrase
  recovery — losing it means a new identity. Point `XDG_CONFIG_HOME`
  elsewhere to test with a throwaway identity.
- **Dev screenshots**: press F12 in the client, or run with
  `CIVORA_SCREENSHOT=<path> CIVORA_SCREENSHOT_DELAY=<secs>` to auto-save
  one screenshot after startup (used for scripted verification; X11
  tools cannot capture the Vulkan window).
- **Two instances on one machine** (P2P testing) need distinct
  identities — the per-author sequence numbers collide otherwise. Use
  `--key-file` (or `CIVORA_KEY_FILE`) to point each instance at its own
  key:

  ```
  # terminal 1
  CIVORA_PASSPHRASE=a cargo run -p civora-client -- --host --key-file /tmp/civ-a.key
  # prints: listening on /ip4/…/tcp/PORT/p2p/PEERID
  # terminal 2
  CIVORA_PASSPHRASE=b cargo run -p civora-client -- --join /ip4/127.0.0.1/tcp/PORT/p2p/PEERID --key-file /tmp/civ-b.key
  ```

  A bare `--join` (no address) waits for mDNS discovery instead.
- **mDNS is best-effort**: VPNs and multicast-filtering Wi-Fi break it;
  the printed multiaddr + `--join` is the guaranteed path.
- **Sync consistency limits (M3)**: gossip is best-effort and there is
  no global action order yet, so concurrent conflicting edits of the
  same voxel can briefly diverge worlds. Beacons detect this within ~5 s
  and the yielding peer resyncs automatically. The cell-committee
  milestone replaces this with proper validated ordering.