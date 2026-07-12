# Plan: "Asset hot patch" milestone plan document

## Context

"Asset hot patch" is **Milestone 9** in PLAN.md's build order (line ~386). M7 (patch packs) and M8 (wasm ABI) have plan documents in `plans/` but are **not yet implemented**; M9 has no plan document at all — it's the only near-term build-order item without one. Following the repo convention (plans are drafted ahead of implementation, referencing planned APIs — M8's plan was written the same way), the deliverable of this task is:

1. Create **`plans/asset-hot-patch.md`** containing the milestone design below.
2. Update PLAN.md's build-order line `- [ ] Asset hot patch` → `- [ ] Asset hot patch (plans/asset-hot-patch.md)` (matching how the unchecked M7/M8 items carry their plan links).

No code is written in this task. The design below was verified against the actual code: `render.rs` uses one shared `StandardMaterial` (`ChunkMaterial`, render.rs:30) with vertex colors `block_color * shade` and no UVs; `BlockId::PLACEABLE` is exactly grass/dirt/stone/plank/glass (ids 1–5); `ProposalKind::AssetPatch`, `asset_cids`, `activation_epoch`, and the `AssetPatchHasCode` validation already exist in civora-governance.

User-confirmed scope decisions (already made, baked into the document):
- **Asset kinds v1**: PNG images + OGG/Vorbis audio (bevy `png`/`vorbis` features already enabled).
- **Demo surface**: retexture terrain blocks (terrain switches from vertex-color to textured rendering).
- **Asset typing**: magic-byte sniffing — no manifest, no proposal format change, no PROTO_VERSION bump.

---

## Content of `plans/asset-hot-patch.md`

# Milestone 9: Asset hot patch

## Context

PLAN.md build-order item 9, after M7 (content-addressed patch packs, plans/patch-packs.md) and M8 (wasm plugin ABI, plans/wasm-abi.md) — **implement both first**; this plan builds on M6's `LedgerStore`/`EpochClock`, M7's `BlobStore`/`ContentStore`/`PackTracker`/`track_pack`/`CIVORA_TEST_VOTE`, and leaves M8's F9 GameplayCode demo untouched (M8 decision 11: "M9 re-introduces an asset-only variant when it needs one"). Today an accepted `AssetPatch` proposal's blobs land hash-verified in every peer's store and stop there. This milestone gives them meaning: at the proposal's `activation_epoch` boundary (PLAN.md "Hot patching": accepted → fetch → verify → activates at epoch N), every peer decodes the pack's assets and applies them live — an image asset **retextures the terrain**, an audio asset plays an activation sound. PLAN.md rates asset patches "Easy" and demands a rollback plan; both are addressed honestly below. **No wasm runs, no live world mutation** — that is M10.

User-confirmed decisions: **asset kinds v1 = PNG images + OGG/Vorbis audio**, decoded into Bevy `Assets<Image>` / one-shot audio playback; **the demo surface is terrain retexturing** — an activated image becomes the terrain texture atlas; **asset typing by magic-byte sniffing** (PNG 8-byte signature / `OggS`) — no manifest, no proposal format change, no `PROTO_VERSION` bump; unknown formats are recorded as unsupported, never a hard error; **activation strictly at the `activation_epoch` boundary**, with late pack completion activating immediately-but-late, and restart deriving all state from ledger + store (nothing new persisted).

## Key decisions

1. **Terrain becomes *always textured* — activation is a texture swap, never a remesh.** The mesher change lands once, statically: emit `ATTRIBUTE_UV_0` on every vertex and demote vertex colors to *shade only* (`[shade, shade, shade, 1]`); block tint moves into a **built-in 5×1-pixel atlas** generated from `block_color()` at startup and set as the shared material's `base_color_texture` from frame one. Bevy PBR multiplies `base_color × base_color_texture × vertex color`, so the result is pixel-identical to today's `block_color * shade` look (±1/255 quantization). Activating a patch = `materials.get_mut(...)` swap of one `Handle<Image>` on the one shared `StandardMaterial` — every chunk entity updates, **zero remeshing, zero vertex data touched**. This also dissolves the tint-times-texture problem by construction: vertex colors never carry tint again. Rejected: mode-dependent meshing (remesh-all on every activation/rollback, two mesh layouts to test) and a second material (two handles to keep in sync across chunk spawns).
2. **Single-atlas convention: an AssetPatch image *is* a terrain atlas iff its dimensions say so.** A horizontal strip of `TERRAIN_ATLAS_TILES = 5` square tiles — one per solid block, tile index = `BlockId.0 - 1`, order grass, dirt, stone, plank, glass (the `PLACEABLE` order). Accepted iff `width == 5 * height` and `1 <= height <= MAX_ATLAS_TILE_PX` (1024; caps GPU upload at ~20 MiB RGBA). Tile size K = height is free — the built-in atlas is K=1, the demo K=8, artists use K=16/32. Bare 32-byte cids need no per-block mapping metadata: the layout *is* the contract. A PNG with other dimensions is recorded `NotAnAtlas`, not failed.
3. **Nearest sampling + fixed UV inset.** Patched atlases decode via the menu.rs `Image::from_buffer` pattern with `ImageSampler::nearest()` (bevy defaults to linear — wrong for tile strips: edge texels bleed across tiles) and `is_srgb = true` (artist PNGs are sRGB). Under nearest sampling any strictly interior UV never samples a neighboring tile, so the mesher insets each face's UV rect by `ATLAS_UV_INSET = 1/128` of one tile (safe for every K, crops an invisible 0.8% border). Runtime images get no mipmaps, so no mip bleeding. The built-in atlas is `TextureFormat::Rgba8Unorm` (**linear**, not sRGB) with raw `block_color` bytes — vertex colors were linear-interpreted, so this preserves the current look exactly; the sRGB/linear split is deliberate and documented in code.
4. **Asset typing = sniffing, in a pure function.** `sniff(bytes) -> AssetKind { Png, Ogg, Unknown }`: PNG = leading `[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]`, Ogg = leading `b"OggS"`. `Unknown` is a recorded per-asset outcome. A blob that sniffs PNG but fails bevy's decoder (`Image::from_buffer` returns `Err`) is recorded `Failed { reason }` — decode failures **never** crash and never poison the rest of the pack.
5. **Activation driver = a 1 Hz polling scan, not new accept-time hooks.** Unlike M7 (which needed fetches fired *at* accept), activation is time-gated anyway, so `activate_asset_patches` (Update, InGame, 1 s `Local` timer — the `evaluate_voting_windows` pattern) scans `LedgerStore` entries with `kind == AssetPatch`, cross-checks `PackTracker::get(id).complete()` and `EpochClock::now_epoch() >= activation_epoch`, and advances a per-proposal state machine `WaitingForPack → WaitingForEpoch → Active`. No edits to M6/M7 choke points at all; restart needs no special path (the ledger seeds, the first scan applies). Up to ~1 s of cross-peer visual skew is accepted.
6. **"Late" activation is the *normal* case — documented, not special-cased.** In the M6 design the voting window closes *at* `activation_epoch`, so by the time any certificate exists, `now_epoch >= activation_epoch` already holds; the real gate post-acceptance is pack completion, and `WaitingForEpoch` is reachable only under clock skew (a remote certificate arriving while our clock lags). The gate stays — it is the PLAN.md contract and guards skew — and `Active { at_epoch }` records the actual epoch so the UI can show `@ epoch N (activated M)` when they differ. Restart/rejoin: the first scan pass reapplies everything from ledger + store, silently (decision 8) — applied-asset state is **derived, never persisted**.
7. **Precedence: recompute the winner from scratch, latest wins.** Every atlas candidate carries the key `(activation_epoch, ProposalId, index in asset_cids)`; the active atlas is the **max** key over all `Active` patches (later epoch wins; ledger tie → higher proposal id; within one proposal → later cid in the ascending list). On any activation the winner is recomputed over the full set and applied — idempotent, arrival-order-independent, and trivially restart-stable (same inputs ⇒ same winner on every peer). If the winning candidate failed to decode it simply isn't a candidate; an empty candidate set falls back to the built-in atlas.
8. **Sounds play once, at live activation only.** Each OGG asset in an activating patch becomes `AudioSource { bytes }` (constructed directly — no `AssetServer`, matching the repo's no-asset-files rule) and is spawned as `(AudioPlayer(handle), PlaybackSettings::DESPAWN)` — fire-and-forget, entity despawns after playback. The first scan pass of a session (ledger seeding) applies patches with `live = false`: **images yes, sounds no** — a restart must not replay the whole ledger's chime history. No persistent soundboard in v1. A blob that sniffs `OggS` but is corrupt vorbis fails inside rodio at playback (bevy warns, nothing crashes); we record `Sound { played: true }` optimistically — best-effort, listed as a known limit.
9. **Scope: only `kind == AssetPatch` proposals activate.** `asset_cids` on GameplayCode/NewContent/etc. stay inert in v1 (M8's F9 sample carries binary assets — those must not start retexturing terrain). Matches the voting-rules table (asset-only patch = simple majority) and M8 decision 11.
10. **Rollback v1 = the built-in atlas fallback, with no automatic trigger.** The deactivation path exists and is exercised (winner selection over an empty set → built-in atlas; unit-tested), but nothing in v1 revokes an accepted ledger entry, so nothing invokes it at runtime. `RollbackPlan::RevertToLastSignedSnapshot` remains unexecuted (as in M6–M8). Stated bluntly in PLAN.md known limits; manual rollback = remove the ledger entry file-side and restart.
11. **Demo PNG is generated by a hand-rolled encoder; demo OGG is a checked-in CC0 fixture.** Bevy's `png` feature is decode-side only (no public encode API), and pulling `image`/`png` as a direct dep for a demo is against house economy — so `encode_rgba_png()` hand-rolls a minimal PNG writer: RGBA8, filter-0 rows, zlib with **stored** (uncompressed) deflate blocks, hand-rolled CRC-32 and Adler-32 (~90 lines; the M7 base32 / M8 wasm-scan precedent), unit-tested by decoding its output back through `Image::from_buffer`. Hand-rolling a valid Ogg/Vorbis stream is *not* realistic — a ~0.3 s sine chime (`ffmpeg -f lavfi -i "sine=frequency=660:duration=0.3" -c:a libvorbis`, ~4 KiB, authored by us, CC0) is checked in at `crates/civora-client/assets/patch-chime.ogg` with a `REUSE.toml` override annotation (`CC0-1.0`), embedded via `include_bytes!` like the logo.
12. **Demo trigger: F10 + `CIVORA_TEST_ASSET=1`, F9 untouched.** F9 stays M8's GameplayCode sample; F10 publishes an AssetPatch sample whose atlas PNG varies per press (counter `n` perturbs tile pixels → distinct cid → distinct proposal). `CIVORA_TEST_ASSET=1` auto-publishes one ~3 s after startup with an author auto-yes (the `CIVORA_TEST_PROPOSAL` pattern); composes with M7's `CIVORA_TEST_VOTE=1` on the joiner.
13. **No new dependencies.** Decode = bevy `png`/`vorbis` features already in Cargo.toml; encode = hand-rolled; sniffing = byte compares. The whole milestone is client-side — governance, net, sim, kernel crates are untouched (`ProposalKind::AssetPatch`, validation, and `referenced_cids()` already exist).

## Step 1 — client: `assets.rs` pure core (new module, `mod assets;` in main.rs)

```rust
/// One tile per solid block, BlockId 1..=5, in PLACEABLE order.
pub const TERRAIN_ATLAS_TILES: u32 = 5;
pub const MAX_ATLAS_TILE_PX: u32 = 1024;
/// UV inset as a fraction of one tile; with nearest sampling any interior
/// point stays inside its tile.
pub const ATLAS_UV_INSET: f32 = 1.0 / 128.0;

pub enum AssetKind { Png, Ogg, Unknown }
pub fn sniff(bytes: &[u8]) -> AssetKind;
/// Some(tile_px) iff width == 5 * height and 1 <= height <= 1024.
pub fn atlas_tile_px(width: u32, height: u32) -> Option<u32>;
/// Inset UV rect (min, max) of `block`'s tile in the 5-tile strip.
/// Ids outside 1..=5 map to tile 0 (documented; mesher only sees solid blocks).
pub fn tile_uv_rect(block: BlockId) -> ([f32; 2], [f32; 2]);
/// Minimal PNG writer: RGBA8, filter 0, stored-deflate zlib, hand-rolled
/// CRC-32/Adler-32. Demo + test tool, not a general encoder.
pub fn encode_rgba_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8>;
```

Inline tests: sniff golden vectors (PNG signature, `OggS`, truncated 3-byte buffer, empty, JPEG magic → `Unknown`); `atlas_tile_px` (5×1 → 1, 40×8 → 8, 64×64 → None, 5120×1024 → 1024, 5125×1025 → None); `tile_uv_rect` (rects strictly inside `[i/5, (i+1)/5] × [0,1]`, disjoint, ascending in block id, out-of-range ids → tile 0); `encode_rgba_png` round-trip: encode a 10×2 gradient, decode via `Image::from_buffer(..., ImageType::Extension("png"), ...)`, assert dimensions and exact pixel bytes (this test is the encoder's correctness proof — `from_buffer` is CPU-side, no App/GPU needed).

## Step 2 — client: always-textured terrain (`render.rs`)

- `ChunkMaterial` becomes public and grows the fallback handle:

```rust
#[derive(Resource)]
pub struct TerrainMaterial {
    pub material: Handle<StandardMaterial>,
    /// The 5x1 block_color fallback; the material's texture when no patch atlas is active.
    pub builtin_atlas: Handle<Image>,
}
fn builtin_atlas() -> Image   // 5x1, TextureFormat::Rgba8Unorm (linear!), nearest sampler,
                              // pixels = round(block_color * 255); doc-comment the sRGB split
```

- `setup_lighting_and_material` gains `ResMut<Assets<Image>>`, builds the built-in atlas via `Image::new` + `image.sampler = ImageSampler::nearest()`, and sets `base_color_texture: Some(builtin.clone())` on the material.
- `build_chunk_mesh`: add `uvs: Vec<[f32; 2]>` emitted for every vertex — per face, the four `FACES` corners map through a fixed corner→tile-local table `FACE_UVS: [[f32; 2]; 4] = [[0.,1.],[1.,1.],[1.,0.],[0.,0.]]` scaled into `tile_uv_rect(block)` (orientation is cosmetic and pinned by the table); `colors` become `[shade, shade, shade, 1.0]`. Insert `Mesh::ATTRIBUTE_UV_0`. Update the module doc ("texture atlases come later" — they came).
- `block_color` stays public unchanged (hotbar swatches keep using it).
- Inline test (no GPU needed — `Mesh` is CPU data): mesh a `VoxelWorld::flat(0)` chunk, assert `ATTRIBUTE_UV_0` present with same length as positions, every UV inside its block's inset rect, and colors are grayscale shades.

## Step 3 — client: activation state machine (`assets.rs`)

```rust
const EVAL_INTERVAL_SECS: f32 = 1.0;
pub const MAX_DETAIL_ASSET_ROWS: usize = 8;   // mirrors M7's blob-row cap

pub struct AssetPatchPlugin;                   // init AssetPatches; activate_asset_patches
                                               // in Update, InGame, after M7's pack systems
#[derive(Resource, Default)]
pub struct AssetPatches {
    patches: BTreeMap<ProposalId, PatchStatus>,
    active_atlas: Option<ActiveAtlas>,
}
pub struct PatchStatus { pub phase: PatchPhase, pub assets: Vec<AssetOutcome> }
pub enum PatchPhase {
    WaitingForPack,
    WaitingForEpoch,                 // clock-skew guard; normally skipped (decision 6)
    Active { at_epoch: u64 },
}
pub enum AssetOutcome {
    Atlas { cid: Cid, tile_px: u32, handle: Handle<Image> },   // a candidate
    Sound { cid: Cid },                                        // spawned once (live only)
    NotAnAtlas { cid: Cid, width: u32, height: u32 },
    Unsupported { cid: Cid },                                  // sniffed Unknown
    Failed { cid: Cid, reason: String },                       // decode/store error
}
pub struct ActiveAtlas { pub proposal: ProposalId, pub cid: Cid,
    pub activation_epoch: u64, pub at_epoch: u64, pub handle: Handle<Image> }

/// Pure winner selection: max (activation_epoch, proposal, index). Unit-testable.
fn select_atlas(candidates: &[AtlasCandidate]) -> Option<usize>;
```

- `activate_asset_patches` (params: `LedgerStore`, `PackTracker`, `EpochClock`, `ContentStore`, `AssetPatches`, `Assets<Image>`, `Assets<AudioSource>`, `Assets<StandardMaterial>`, `TerrainMaterial`, `Commands`, `Time`, `Local<f32>` timer, `Local<bool>` seeded): every 1 s, for each ledger entry with `kind == AssetPatch` not yet `Active` — phase per pack-complete + epoch checks; on activation, for each `asset_cids` entry: `store.get` (miss/corrupt → `Failed`, `warn!`), `sniff`, PNG → `Image::from_buffer(bytes, Extension("png"), CompressedImageFormats::NONE, true, ImageSampler::nearest(), RenderAssetUsages::RENDER_WORLD)` (the menu.rs pattern; `Err` → `Failed`, never panic) → `atlas_tile_px` gate → `Atlas` or `NotAnAtlas`; OGG → `Sound`, and if `live` (not the first pass), `commands.spawn((AudioPlayer(audios.add(AudioSource { bytes: bytes.into() })), PlaybackSettings::DESPAWN))` (verify the public `bytes` field on bevy 0.19 at implementation time; if not constructible, fall back to `Assets::add` of a decoded wrapper). After any activation: recompute `select_atlas` over all Active patches, and if the winner changed set `materials.get_mut(&terrain.material).base_color_texture = Some(winner.handle)` (or `terrain.builtin_atlas` when none), `info!("asset patch {} active at epoch {} (atlas {})", ...)`.
- First invocation sets `*seeded = true` after processing with `live = false` — the restart/rejoin path: images reapply, sounds stay silent.

Inline tests (plain structs, fake epochs, no Bevy app): phase transitions — pack incomplete holds `WaitingForPack`; complete + `now >= activation` goes straight to `Active` recording `at_epoch = now` (the late/normal case); complete + `now < activation` holds `WaitingForEpoch` then activates at the boundary. `select_atlas`: later `activation_epoch` wins; equal epochs → higher `ProposalId`; same proposal → later index; empty → None (built-in fallback).

## Step 4 — client: UI (`voting.rs`, `hud.rs`)

- Detail pane, `kind == AssetPatch` and status Accepted, after M7's pack rows (new `Res<AssetPatches>` param threaded into the detail writer): a phase line — `assets  waiting for pack` | `assets  waiting for epoch N` | `assets  active @ epoch N` / `assets  active @ epoch N (activated M)` — then up to `MAX_DETAIL_ASSET_ROWS` rows: `  <cid8> png atlas 40x8 - ACTIVE` (winner) / `png atlas 40x8` (candidate, superseded) / `png 64x64 (not an atlas)` / `ogg sound` / `unsupported format` / `failed: <reason>`, then `  + k more`.
- `hud.rs` (new `Res<AssetPatches>` param), one line when the map is non-empty: `assets: atlas <cid8> @ epoch N` when a patch atlas is active, else `assets: builtin atlas (k pending)`. This is the line two peers screenshot-compare (same winning cid on both).

## Step 5 — client: demo path (`debug.rs` + fixture + `REUSE.toml`)

- `crates/civora-client/assets/patch-chime.ogg` checked in (decision 11); `REUSE.toml` gains an override annotation for that path (`CC0-1.0`).
- `debug.rs`:

```rust
const CHIME_OGG: &[u8] = include_bytes!("../assets/patch-chime.ogg");
/// 40x8 (K=8) deterministic tile strip: block_color base per tile, a darker
/// 1px border, and a checker perturbed by `n` so each press yields a new cid.
fn demo_atlas_rgba(n: u32) -> Vec<u8>;
fn sample_asset_proposal(author, n, now_epoch, store: &BlobStore) -> Proposal
// kind AssetPatch; puts source/build/tests text blobs (M7 pattern) +
// encode_rgba_png(40, 8, &demo_atlas_rgba(n)) + CHIME_OGG; asset_cids =
// sorted [png_cid, ogg_cid]; wasm/migrations empty (validation demands it);
// activation_epoch = now + DEMO_ACTIVATION_EPOCHS; validate().expect(...)
```

- F10 handler + `CIVORA_TEST_ASSET=1` auto-publish (3 s delay, author auto-yes) — both reuse M7's sample-publish plumbing with the asset sample; F9/`CIVORA_TEST_PROPOSAL` (M8 GameplayCode) and `CIVORA_TEST_VOTE` untouched. 5 blobs per pack keeps the multi-blob fetch UI exercised.

## Step 6 — PLAN.md + plans doc

- Check off item 9 (`plans/asset-hot-patch.md` + done date); save this plan there.
- Status section: always-textured terrain + built-in atlas trick (no remesh on activation), the 5-tile atlas convention (the artist contract: strip PNG, `w = 5h`, grass/dirt/stone/plank/glass, nearest-sampled), sniffing, the 1 Hz activation scan + state machine, latest-wins precedence, live-only sounds.
- Build notes: F10 / `CIVORA_TEST_ASSET`, the atlas dimension rule, "late activation is normal" (window closes at `activation_epoch`), restart reapplies silently, updated two-instance recipe.
- Known limits list (below), including the honest rollback statement PLAN.md's "Every patch needs a rollback plan" demands.

## Implementation order

1. `assets.rs` pure core (sniff, atlas math, PNG encoder) + tests
2. `render.rs` always-textured terrain + mesher test (visual no-op — verify by eye before anything activates)
3. `AssetPatches` + `activate_asset_patches` + winner application + audio
4. UI (voting detail rows, HUD line)
5. `debug.rs` F10/`CIVORA_TEST_ASSET` + chime fixture + `REUSE.toml`
6. PLAN.md + plans doc + verification

(1 and 2 are independent; 3 needs both; 4–5 need 3.)

## Verification

- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.
- **Step-2 regression check first**: run the client before step 3 exists — terrain must look identical to M8 (the built-in atlas reproduces vertex tinting).
- **Two-instance manual demo** (M7 recipe — distinct key/ledger/store dirs):
  ```
  CIVORA_PASSPHRASE=a CIVORA_EPOCH_SECS=5 cargo run -p civora-client -- --host \
    --key-file /tmp/civ-a.key --ledger-file /tmp/civ-a.ledger --store-dir /tmp/civ-a-store
  CIVORA_PASSPHRASE=b CIVORA_EPOCH_SECS=5 CIVORA_TEST_VOTE=1 cargo run -p civora-client -- \
    --join /ip4/127.0.0.1/tcp/PORT/p2p/PEERID \
    --key-file /tmp/civ-b.key --ledger-file /tmp/civ-b.ledger --store-dir /tmp/civ-b-store
  ```
  Host presses F10, joiner auto-votes; at window close both flip `[accepted]`, the joiner's pack counts to 5/5, and within ~2 s **both terrains retexture to the same 8px-tile atlas** and both play the chime once; both HUDs show `assets: atlas <cid8> @ epoch N` with the same cid. Press F10 again → a second patch with a later `activation_epoch` supersedes the first on both peers (latest-wins). Restart the joiner → terrain comes back textured with the winning atlas, **no chime** (seed pass is silent). Negative: kill the host pre-fetch → joiner sits in `waiting for pack` with the built-in atlas; restart host → 10 s retry completes the pack → activation fires late, detail shows `(activated M)`.
- **Scripted screenshot**: host `CIVORA_EPOCH_SECS=2 CIVORA_TEST_ASSET=1 CIVORA_SCREENSHOT=/tmp/civ-m9-a.png CIVORA_SCREENSHOT_DELAY=25 … --host …`; joiner `CIVORA_EPOCH_SECS=2 CIVORA_TEST_VOTE=1 CIVORA_SCREENSHOT=/tmp/civ-m9-b.png CIVORA_SCREENSHOT_DELAY=25 … --join …` — both screenshots show textured (checker-tiled, no longer flat-color) terrain and the identical `assets: atlas <cid8>` HUD line.

## Known accepted limits (state in PLAN.md)

- Only `kind == AssetPatch` proposals activate; `asset_cids` on other kinds stay inert until a later milestone defines their meaning.
- One global terrain-atlas slot, latest-accepted-wins; no per-block patches, no named assets, no mesh/biome/config asset classes — bare cids + dimension convention is the whole addressing scheme in v1.
- Rollback v1 is the built-in-atlas fallback path with **no automatic trigger**: nothing revokes a ledger entry yet, and `RollbackPlan::RevertToLastSignedSnapshot` remains unexecuted (as since M6). Manual rollback = remove the ledger entry and restart.
- Every activation is "late" by ≥ 0 epochs by construction (the voting window closes *at* `activation_epoch`); the epoch gate guards clock skew, not scheduling.
- PNG alpha is ignored (material stays opaque); atlas tiles cap at 1024 px; superseded atlas candidates stay decoded in `Assets<Image>` (bounded by 16 MiB/blob × asset count).
- Sounds are best-effort: `OggS` sniffing cannot pre-validate vorbis; a corrupt stream warns inside bevy_audio at playback and is still recorded as played. Sounds never replay on restart (by design).
- Hotbar swatches keep the built-in `block_color`s — they do not sample the active atlas.
- Cross-peer switchover skew up to ~1 s (independent 1 Hz scans); acceptable for a visual patch, irrelevant to determinism (no sim state involved).

---

## Execution steps (this task)

1. Write the document above (everything between the `---` markers, starting at `# Milestone 9: Asset hot patch`) to `plans/asset-hot-patch.md`.
2. Edit PLAN.md build order (~line 386): `- [ ] Asset hot patch` → `- [ ] Asset hot patch (plans/asset-hot-patch.md)`.
3. Verification: `ls plans/` shows the new file alongside patch-packs.md / wasm-abi.md; `grep "asset-hot-patch" PLAN.md` shows the linked build-order item; no other files touched (`git status` shows exactly two changes). Commit only if the user asks.
