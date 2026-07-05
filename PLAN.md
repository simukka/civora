
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
- [ ] P2P lobby and world cell sync
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