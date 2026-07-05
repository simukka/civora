# Civora Milestone 2: Player Identity and Signed Actions

## Context

PLAN.md's build order item 2 (after the done voxel client): **"Player identity and signed actions"**. Per README/AGENTS, every world mutation must eventually be a signed, verifiable action gossiped to cell committees; the Reality Kernel's jobs include signature verification and player-key protection, and the Local Data Layer holds "local player keys". Milestone 1 deliberately built the seam for this: all edits are plain-data `Action`s (`crates/civora-sim/src/action.rs`) drained through `tick::step` by `drain_action_queue` in `crates/civora-client/src/sim_bridge.rs` — the client never mutates the world directly.

This milestone gives the player a persistent Ed25519 identity (Ed25519 = what libp2p peer identities use, keeping the P2P milestone compatible) and makes every applied action a **signed, verified, logged** action, proving the loop locally before networking exists.

Per user decisions: **passphrase-encrypted key file** (not plaintext, not ephemeral) and **in-memory signed action log + replay proof** (no log persistence yet, matching milestone 1's in-memory-only choice).

## Deliverable

- First launch: client prompts for a new passphrase (terminal, entered twice), generates an Ed25519 keypair, saves it encrypted to the OS config dir. Later launches: prompts for the passphrase (3 attempts) and unlocks the same identity. `CIVORA_PASSPHRASE` env var bypasses the prompt for scripted runs.
- Every break/place is signed with the player's key, **verified before being applied** (the kernel gate), and appended to an in-memory signed action log with a per-author monotonic sequence number.
- Debug HUD shows the player ID (short hex of the public key).
- Tests prove end-to-end: verify every signature + seq in a log, replay it onto a fresh world, get a content hash identical to the directly-mutated world; tampered actions/signatures/authors and replayed seqs are rejected.

## New crate: `crates/civora-identity`

Depends on `civora-sim` plus pure-Rust crypto (all cross-platform, no CI changes needed):
`ed25519-dalek = "2.2"` (feature `rand_core`), `rand_core = { version = "0.6", features = ["getrandom"] }`, `argon2`, `chacha20poly1305` (XChaCha20-Poly1305). Dev-dep: `tempfile`.

Modules:

- `identity.rs` — `PlayerId([u8; 32])` (verifying-key bytes; `Display` = hex, `short()` = first 8 hex chars) and `Identity` wrapping `SigningKey`: `generate()`, `player_id()`, `sign(action, seq) -> SignedAction`.
- `signed.rs` — `SignedAction { author: PlayerId, seq: u64, action: Action, signature: [u8; 64] }` with `verify() -> Result<(), VerifyError>`. Signing payload with domain separation: `b"civora.action.v1" || author || seq(u64 LE) || Action::encode(...)`.
- `log.rs` — `ActionLog(Vec<SignedAction>)`: `append` (rejects bad signature or non-increasing seq per author), `verify_and_replay(&self, &mut VoxelWorld) -> Result<Vec<ChunkPos>, VerifyError>` re-checking every entry and driving `tick::step`.
- `keyfile.rs` — `save_encrypted(path, &Identity, passphrase)` / `load_encrypted(path, passphrase) -> Result<Identity, KeyfileError>`. Format: magic `CIVKEY1` || Argon2id salt (16) || XChaCha nonce (24) || AEAD ciphertext of the 32-byte seed. Argon2id (default params) derives the AEAD key. Unix: write file with mode 0600. Wrong passphrase → typed error (AEAD auth failure), never a panic. **No interactive I/O in this crate** — passphrase arrives as an argument (keeps the kernel-ish crate testable and UI-free).

## `civora-sim` changes (stays dependency-free)

`action.rs`: canonical byte encoding for signing/gossip — `Action::encode(&self, out: &mut Vec<u8>)` and `Action::decode(&[u8]) -> Option<Action>`. Hand-rolled fixed format (tag byte, i32 LE coords, block u8); no serde, so the encoding is canonical by construction.

## Client changes (`crates/civora-client`)

New deps: `civora-identity`, `dirs = "6"`, `rpassword`.

- `main.rs` — before `App::run()`: resolve `dirs::config_dir()/civora/identity.key`; passphrase from `CIVORA_PASSPHRASE` or `rpassword` terminal prompt (create-with-confirmation on first run, unlock with 3 attempts otherwise; exit with a clear error on failure). Insert `LocalIdentity { identity, next_seq: u64 }` and `ActionLog` as resources. Prompting happens in `main` so it runs before the window opens.
- `sim_bridge.rs` — `drain_action_queue` becomes the kernel gate: drain raw `Action`s → sign each with `LocalIdentity` (incrementing `next_seq`) → `ActionLog::append` (which verifies signature + seq) → only verified actions go to `tick::step`. A rejected action logs `warn!` and is dropped — world state only ever changes via a verified `SignedAction`.
- `hud.rs` — debug overlay gains an `id <short-hex>` line from `LocalIdentity`.

## Tests

- `civora-sim/tests/sim.rs`: `Action` encode/decode round-trip for both variants incl. negative coords; decode rejects truncated input and unknown tags.
- `civora-identity/tests/identity.rs`:
  - sign → verify round-trip
  - tampering: flipped byte in action bytes / signature / author each fails verify
  - log rejects a reused or decreasing seq (anti-replay)
  - **replay proof**: build a flat world, apply a mixed action script directly via `tick::step`; separately sign+append the same script to an `ActionLog` and `verify_and_replay` onto a fresh flat world; assert equal `content_hash()`
  - keyfile: save → load round-trip recovers the same `PlayerId`; wrong passphrase → error; corrupted file → error (tempfile dir)

## Out of scope (later milestones)

P2P gossip of signed actions, eligibility roster / anti-Sybil, votes and certificates, log persistence to disk, world persistence, passphrase change/key rotation UX, in-game passphrase UI (terminal prompt only for now), Wasm key isolation.

## Verification

1. `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --check` — all green (these are the CI gates; no `ci.yml` changes needed since it already builds/tests the whole workspace and all new deps are pure Rust).
2. First run: `CIVORA_PASSPHRASE=test cargo run -p civora-client` with `CIVORA_SCREENSHOT` — key file appears under the config dir; screenshot shows `id <hex>` in the HUD; break/place still works (actions now flow through sign→verify→apply).
3. Second run with the same passphrase logs the **same** player ID; a run with a wrong passphrase (no env, bad input) exits with a clear error, not a panic.

## Wrap-up

Update PLAN.md: check off the build-order item, add a Milestone 2 entry to Status, and note the key-file location + `CIVORA_PASSPHRASE` in Build notes. Commit and push (CI must pass).
