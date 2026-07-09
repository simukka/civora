# Milestone 7: Content-addressed patch packs

## Context

PLAN.md build-order item 7, directly after Milestone 6 (accepted proposal ledger, plans/accepted-proposal-ledger.md — **implement M6 first**; this plan builds on its `Ledger`, `FinalityCertificate`, `apply_certificate`, `EpochClock`, and startup seeding). Today `civora_governance::Cid` is a placeholder raw SHA-256 digest, proposal manifests reference content nobody can fetch, and the F9 sample hashes fake label strings. This milestone makes accepted proposals' content real: every referenced artifact lands, hash-verified, in every peer's **local content-addressed store** ("Local Data Layer — content-addressed assets"), fetched over a new `/civora/fetch/1` protocol. **Nothing is loaded or applied** — wasm ABI is milestone 8, asset hot patch is milestone 9.

User-confirmed decisions: **manifests keep gossiping whole** (tag 2 unchanged, 256 KiB cap stays; announce-then-fetch deferred — only referenced artifacts move by content fetch); **fetch fires only after acceptance** (the finality certificate / ledger append is the trigger; voters see manifest metadata only); **CIDv1 is presentation-only** (`Cid` stays a 32-byte digest in every wire/persisted encoding — no proposal format bump; CIDv1 text helpers added, base32 hand-rolled, no new dependency).

## Key decisions

1. **CIDv1 text form**: `to_cid_string()` renders `'b' + base32lower_nopad(0x01 0x55 0x12 0x20 || digest)` (CIDv1, raw codec 0x55, sha2-256 multihash), `from_cid_string()` is the strict inverse. `Display` stays hex for logs; `short()` unchanged. Golden vectors verified externally: `Cid::of(b"")` → `bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku` (the well-known IPFS empty raw block, 59 chars), `Cid::of(b"civora")` → `bafkreiauohq3h7rrn2a5vtmxenkiz4gdjiyvy3w25t4crlcrdohjdme44m`.
2. **Blob store lives in `crates/civora-governance/src/store.rs`** next to `Cid` + sha2, following `ledger.rs`'s std::fs precedent (net integration tests can use it without the client). Git-style layout: `root/<hex[0..2]>/<hex64>`, file content = the raw blob bytes.
3. **Blob files carry no magic prefix** — deliberate, documented deviation: the filename *is* sha256(content) and `get()` re-hashes on read, which subsumes what a version byte buys; a prefix would break `sha256sum file == filename` self-verification. Blobs are public data: default permissions, no 0o600.
4. **`MAX_BLOB_BYTES = 16 MiB`**, enforced on put, on read, and by the fetch codec cap. Per-blob only; no total-pack cap in v1 (documented limit — voters see manifest cid counts before approving).
5. **New `/civora/fetch/1` request-response protocol**, second codec with independent caps (`MAX_FETCH_REQUEST_BYTES = 64`, `MAX_FETCH_RESPONSE_BYTES = MAX_BLOB_BYTES + 16`) so sync's 64 MiB cap never applies. **One blob per request**; pipelining = multiple in-flight requests. Additive protocol — **no `PROTO_VERSION` bump** (stays at M6's 2), no gossip change.
6. **The event loop verifies bytes-hash-to-cid** (the `on_join_response` content-hash precedent: the net layer owns content-hash checks, signatures stay client-side). A hash mismatch is treated like `NotFound`: mark the peer tried, retry the next connected peer.
7. **Serving round-trips through the client** (ProvideSnapshot pattern): inbound request → `NetEvent::BlobRequested { request_id, cid }` → `NetCommand::ProvideBlob { request_id, bytes: Option<Vec<u8>> }` → `Blob`/`NotFound`. `next_request_id` is shared with snapshots. No `live` gate — the store exists before the world syncs.
8. **Outbound fetch tracking**: `pending_fetches: HashMap<OutboundRequestId, FetchState { cid, tried: Vec<PeerId> }>`. Sync and fetch are separate behaviours so their request ids can't collide with `join_request`. On NotFound / outbound failure / hash mismatch → retry the next connected peer not yet tried; exhausted → `BlobFetchFailed { cid, reason }`. Duplicate `FetchBlob` for an in-flight cid is dropped (debug). Zero peers → immediate failure (client retry timer re-requests).
9. **One accept-time choke point**: every path that lands a ledger entry — local certification, `RemoteCertificate` via `apply_certificate` (join-synced certificates included), startup seeding from the persisted ledger — calls `packs::track_pack`. `SyncResponse::Accept` does **not** carry blob bytes; joiners fetch post-accept like everyone else.
10. **`Proposal::referenced_cids()`** returns every referenced cid (source bundle, build manifest, wasm modules, assets, migrations, governance rule module, test results, reverse migrations), deduplicated via `BTreeSet`, ascending. `git_commit_hash` is provenance, not fetched.
11. **Client `PackTracker`** resource: per accepted `ProposalId`, `PackStatus { cids, missing, in_flight, failed }`; a fetched blob clears from *every* pack containing it; a 10 s retry system re-requests `missing − in_flight` while peers exist. Failures decay back into eligibility on the next timer tick.
12. **Demo becomes real content**: the F9 sample `put()`s five deterministic blobs (source/build/tests text + two binary assets) into the local store at propose time and references the real cids. New env `CIVORA_TEST_VOTE=1` auto-votes yes on every open proposal (the joiner side of scripted runs). **Two instances on one machine need distinct `--store-dir`s** or the fetch demo proves nothing.
13. **Stale docs updated**: the `MAX_GOSSIP_BYTES` comment in behaviour.rs and PLAN.md's M5/M4 build notes (announce-then-fetch is deferred, not delivered; Cid now has a real CIDv1 text form, digest remains the wire form).

## Step 1 — governance: CIDv1 helpers (`crates/civora-governance/src/cid.rs`)

```rust
/// version 1 (0x01) || raw codec (0x55) || sha2-256 (0x12) || digest len (0x20)
pub const CIDV1_RAW_SHA256_PREFIX: [u8; 4] = [0x01, 0x55, 0x12, 0x20];
/// 'b' multibase prefix + base32(36 bytes) = 1 + ceil(288/5) = 59.
pub const CID_STRING_LEN: usize = 59;

impl Cid {
    pub fn to_cid_string(&self) -> String;           // "bafkrei…"
    pub fn from_cid_string(s: &str) -> Option<Cid>;  // strict inverse
}
// private: base32_encode / base32_decode, RFC 4648 lowercase alphabet
// "abcdefghijklmnopqrstuvwxyz234567", no padding
```

`from_cid_string` rejects, in order: length ≠ 59; first char ≠ `'b'`; any non-alphabet char (uppercase, `=`, `0/1/8/9`); non-zero trailing bits (58 symbols = 290 bits carry 288 — the low 2 bits of the last symbol must be 0); header ≠ prefix. Update the module doc (no longer a placeholder).

Inline tests: both golden vectors above (empty block is the external IPFS cross-check, `b"civora"` is the stability pin); round-trip; the rejection list.

## Step 2 — governance: `Proposal::referenced_cids()` (`proposal.rs`)

`pub fn referenced_cids(&self) -> Vec<Cid>` — `BTreeSet` over all eight cid sources (including `RollbackPlan::ReverseMigrations`), sorted ascending. Doc: "the fetch list a peer resolves after the proposal is accepted." Unit test: repeats across lists dedup; governance rule + reverse migrations included; ascending.

## Step 3 — governance: `store.rs` (`BlobStore`)

New `crates/civora-governance/src/store.rs` (+ re-exports in `lib.rs`; `tempfile` dev-dep exists from M6):

```rust
pub const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;

pub struct BlobStore { root: PathBuf }
pub enum BlobStoreError { Io(std::io::Error), TooLarge { len: usize },
    Corrupt { expected: Cid, actual: Cid } }

impl BlobStore {
    pub fn open(root: PathBuf) -> Result<BlobStore, BlobStoreError>;   // create_dir_all
    pub fn put(&self, bytes: &[u8]) -> Result<Cid, BlobStoreError>;    // idempotent
    pub fn get(&self, cid: &Cid) -> Result<Option<Vec<u8>>, BlobStoreError>; // None = missing
    pub fn has(&self, cid: &Cid) -> bool;                              // existence only
    fn path_of(&self, cid: &Cid) -> PathBuf;                           // root/<hex[0..2]>/<hex64>
}
```

- `put`: cap check → hash → existing file returns `Ok(cid)` without rewriting → write `<hex64>.tmp.<pid>` in the shard dir → `fs::rename` (same-dir atomic; pid suffix makes shared-store concurrency safe, and identical content makes races harmless).
- `get`: missing → `Ok(None)`; length > cap or re-hash mismatch → `Err(Corrupt)`. No auto-delete of corrupt files in v1.

Tests in new `crates/civora-governance/tests/store.rs` (tempfile): round-trip incl. empty blob; idempotent put; missing → None; over-cap → TooLarge; flipped byte on disk → Corrupt; `has`; shard layout; no `.tmp.` litter.

## Step 4 — net: wire types (`crates/civora-net/src/wire.rs`)

```rust
pub const MAX_FETCH_REQUEST_BYTES: usize = 64;                      // tag + cid = 33
pub const MAX_FETCH_RESPONSE_BYTES: usize = MAX_BLOB_BYTES + 16;

pub enum FetchRequest { Blob { cid: Cid } }        // 0x00 || cid(32); decode_exact style
pub enum FetchResponse {
    Blob { bytes: Vec<u8> },                       // 0x00 || len(u32 LE) || bytes, len <= MAX_BLOB_BYTES
    NotFound,                                      // 0x01
}
```

Unit tests: round-trips (incl. empty Blob), truncation sweep, trailing byte, unknown tags, over-cap len rejected, encoded request ≤ 64 B.

## Step 5 — net: codec + behaviour

- `codec.rs`: `FETCH_PROTOCOL = StreamProtocol::new("/civora/fetch/1")`; `FetchCodec` mirrors `SyncCodec`, reusing `read_frame`/`write_frame` with the fetch caps. Update the module doc (two protocols).
- `behaviour.rs`: add `pub fetch: request_response::Behaviour<FetchCodec>` (constructed like `sync`, `ProtocolSupport::Full`). Rewrite the `MAX_GOSSIP_BYTES` comment: manifests keep gossiping whole; artifacts move over `/civora/fetch/1`; announce-then-fetch deferred.

## Step 6 — net: lib.rs + event_loop.rs

- `lib.rs`: `NetCommand::FetchBlob { cid }`, `NetCommand::ProvideBlob { request_id, bytes: Option<Vec<u8>> }`; `NetEvent::BlobRequested { request_id, cid }`, `NetEvent::BlobFetched { cid, bytes }` (doc: bytes already hash to cid — checked in the net layer), `NetEvent::BlobFetchFailed { cid, reason }`.
- `event_loop.rs`: `pending_blobs: HashMap<u64, ResponseChannel<FetchResponse>>` (shares `next_request_id`) + `pending_fetches` per decision 8. `start_fetch` picks the first connected player not in `tried` (carrying the last failure reason for the exhausted case); `on_fetch` handles inbound requests (no live gate), responses (hash check → `BlobFetched` | retry), and `OutboundFailure` (also covers peers without `/civora/fetch/1`: `UnsupportedProtocols`). `ProvideBlob` maps `None`/oversize to `NotFound`.

## Step 7 — net: integration tests (`crates/civora-net/tests/sync.rs`)

`TestNode` gains a `BlobStore` over a `TempDir` + a `serve_blob(request_id, cid)` helper.

1. `blob_fetch_round_trips`: host puts ~100 KiB; joiner `FetchBlob`; host serves; joiner gets `BlobFetched`, bytes hash to cid, `put` + `has`.
2. `blob_fetch_not_found_fails_cleanly`: unknown cid → `ProvideBlob { bytes: None }` → `BlobFetchFailed` ("not found"), never `BlobFetched`.
3. `blob_fetch_rejects_hash_mismatch`: host answers wrong bytes → `BlobFetchFailed` ("hash mismatch"), never `BlobFetched`.
4. `fetch_retries_across_peers`: three nodes; only C has the blob; A fetches; the test answers every `BlobRequested` on B and C; A gets `BlobFetched` regardless of try order.

## Step 8 — client: CLI + store resource

- `cli.rs`: `--store-dir PATH`; `CliArgs.store_dir: Option<PathBuf>`.
- New `crates/civora-client/src/packs.rs` (`mod packs;` in main.rs):

```rust
pub const STORE_DIR_ENV: &str = "CIVORA_STORE_DIR";
const RETRY_FETCH_SECS: f32 = 10.0;
const MAX_DETAIL_BLOB_ROWS: usize = 8;

#[derive(Resource)] pub struct ContentStore(pub BlobStore);
pub fn store_dir(overridden: Option<PathBuf>) -> Result<PathBuf, String>
// --store-dir | CIVORA_STORE_DIR | dirs::config_dir()/civora/store (mirrors key/ledger)
```

- `main.rs`: resolve + `BlobStore::open` before the app starts (hard error naming the path); insert `ContentStore` always (offline F9 needs it).

## Step 9 — client: `PackTracker` + orchestration (`packs.rs`)

```rust
#[derive(Resource, Default)]
pub struct PackTracker { packs: BTreeMap<ProposalId, PackStatus> }
pub struct PackStatus { pub cids: Vec<Cid>, pub missing: BTreeSet<Cid>,
    pub in_flight: BTreeSet<Cid>, pub failed: BTreeMap<Cid, String> }
// total()/local()/complete(); counts() -> (complete, syncing) for the HUD
// on_fetched(cid) clears from every pack; on_failed(cid, reason) clears in_flight, records reason

pub fn track_pack(tracker, store, channels: Option<&NetChannels>, id, proposal)
// referenced_cids() -> split by store.has() -> FetchBlob per missing cid
```

- **Call sites (small M6 adjustments)**: `evaluate_voting_windows` after a successful local append; the `RemoteCertificate` arm after `apply_certificate` (have it return the accepted proposal on success — join-synced certificates flow through the same arm); the `OnEnter(AppState::InGame)` seeding system tracks every persisted ledger entry.
- `pump_net_events` arms: `BlobRequested` → `store.get` (Corrupt/Io → warn + `None`) → `ProvideBlob`; `BlobFetched` → `store.put` + `tracker.on_fetched` + info; `BlobFetchFailed` → `tracker.on_failed` + debug. On `Fatal`, clear all `in_flight`.
- System `retry_missing_blobs` (Update, InGame, `run_if(resource_exists::<NetChannels>)`, `Local` timer at 10 s): skip when `PeerRoster` empty; `FetchBlob` each `missing − in_flight`, mark in-flight.

## Step 10 — client: UI (`voting.rs`, `hud.rs`)

- List rows (accepted only): ` pack n/m` suffix.
- Detail (accepted only): `pack   n/m blobs local` (+ `(fetching k, failed j)` or `- complete`), then up to 8 rows of `  <59-char CIDv1> [local|fetching|failed]`, then `  + k more`. This is the CIDv1 presentation surface.
- `hud.rs`: `packs: X complete, Y syncing` when the tracker is non-empty.

## Step 11 — client: demo path (`debug.rs`)

- `sample_proposal(author, n, now_epoch, store)` puts real bytes at propose time: `source`/`build`/`tests` as `format!("civora sample {label} {n}\n")` text, two deterministic binary assets (a few KiB, derived from `n`); asset cids sorted ascending before manifest build; 5 blobs per pack. `git_commit_hash` stays label-derived (provenance only).
- New `CIVORA_TEST_VOTE=1`: auto-votes yes (normal sign → `insert_vote` → `PublishVote` path) on every `Open` proposal not yet voted on. M6's `CIVORA_TEST_PROPOSAL=1` unchanged apart from real blobs.

## Step 12 — PLAN.md + plans doc

- Check off item 7 (`plans/content-addressed-patch-packs.md` + done date); save this plan there.
- Status section: BlobStore layout + no-magic rationale, `/civora/fetch/1` + caps, CIDv1 helpers + golden vector, accept-triggered PackTracker, per-blob UI.
- Build notes: `--store-dir`/`CIVORA_STORE_DIR` (default `~/.config/civora/store`; **two instances need distinct store dirs**), 16 MiB blob cap, 10 s retry, `CIVORA_TEST_VOTE`, updated two-instance recipe; **fix the stale M5 note** (announce-then-fetch deferred, PLAN.md ~line 588) and the M4 Cid note (~line 627).

## Implementation order

1. governance cid.rs → 2. referenced_cids → 3. store.rs + tests → 4. net wire fetch types → 5. codec/behaviour → 6. lib/event_loop → 7. net integration tests → 8. client cli/main/ContentStore → 9. packs.rs + pump arms + M6 call-site hooks → 10. UI → 11. debug.rs → 12. PLAN.md/plans + verification. (1–3 parallel with 4–5.)

## Verification

- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.
- **Two-instance manual demo** (distinct key/ledger/store per instance):
  ```
  CIVORA_PASSPHRASE=a CIVORA_EPOCH_SECS=5 cargo run -p civora-client -- --host \
    --key-file /tmp/civ-a.key --ledger-file /tmp/civ-a.ledger --store-dir /tmp/civ-a-store
  CIVORA_PASSPHRASE=b CIVORA_EPOCH_SECS=5 cargo run -p civora-client -- --join /ip4/127.0.0.1/tcp/PORT/p2p/PEERID \
    --key-file /tmp/civ-b.key --ledger-file /tmp/civ-b.ledger --store-dir /tmp/civ-b-store
  ```
  F9 on host, both press Y; at close both flip `[accepted]`; the joiner's detail counts up to `pack 5/5 blobs local`. Disk proof: every file under `/tmp/civ-b-store` sha256sums to its own filename. Negative: kill the host before the fetch → `fetching/failed`; restart host → the 10 s retry completes the pack. Restart the joiner → pack seeds from ledger + store as 5/5 immediately.
- **Scripted screenshot**: host `CIVORA_EPOCH_SECS=2 CIVORA_TEST_PROPOSAL=1 … --host --store-dir /tmp/civ-a-store`; joiner `CIVORA_EPOCH_SECS=2 CIVORA_TEST_VOTE=1 CIVORA_SCREENSHOT=/tmp/civ-m7.png CIVORA_SCREENSHOT_DELAY=20 … --join … --store-dir /tmp/civ-b-store` — the joiner's screenshot shows HUD `packs: 1 complete, 0 syncing`.

## Known accepted limits (state in PLAN.md)

- Fetching is from directly connected peers only, serial with retry — no provider discovery (DHT), no parallel/bitswap swarming, no resumable transfer.
- No total-pack cap, no pinning, no garbage collection: the store grows monotonically (per-blob 16 MiB cap only).
- Any connected peer can fetch any blob (content is public by design).
- A corrupt store file serves as warn + NotFound; healing is manual deletion.
- Blob files carry no magic prefix (name = sha256(content) is the integrity mechanism — deliberate deviation from the persisted-record house rule).
- Nothing is loaded or applied; content lands verified on disk and stops there until M8/M9.
