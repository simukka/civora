# Milestone 5: Voting UI

## Context

PLAN.md build-order item 5, after Milestone 4 shipped the `SignedProposal` format (no distribution). Requirements from the user: a **simple text in-game UI** where everyone always sees the **number of open proposals** (HUD), pressing a **single key** opens a text **list** of proposals (opt-in), and selecting one opens a **panel with proposal details plus the Voting UI**. User-confirmed scope: P2P proposal gossip + a debug publish key (demoable with two instances), and **signed votes with a live yes/no tally** under a new `civora.vote.v1` domain. Quorum evaluation, finality certificates, and the ledger remain milestone 6; no patch application.

## Key decisions

1. **`Vote` is minimal and fixed-size** (66 bytes: version, proposal_id, voter, choice) in a new `crates/civora-governance/src/vote.rs`; `SignedVote` mirrors `SignedProposal` (sign panics on author mismatch, `verify()` via `civora_identity::verify_payload`). Fixed 130-byte encoding, no length prefix. Version byte included — votes are governance records the M6 ledger will persist.
2. **Revote = latest received replaces** (plain map insert). M5's tally is display-only; doc-comment notes M6 finality must bind votes to an ordering since gossip arrival isn't consistent across peers.
3. **"Open proposal" in M5** = every known proposal passing `SignedProposal::verify()` + `Proposal::validate()`, forever — no epochs/windows/finality exist yet; `activation_epoch` displays as a raw number. Goes in PLAN.md build notes.
4. **One new gossip topic** `civora/1/genesis/0/proposals` carrying `GossipMsg::Proposal` (tag 2) and `GossipMsg::Vote` (tag 3). **No live-gate buffering**: unlike actions, verification is self-contained and store insertion idempotent, so they're emitted to the client even while joining.
5. **Gossipsub `max_transmit_size` raised to 256 KiB** via `ConfigBuilder` (default 64 KiB < worst-case encoded SignedProposal of 192 KiB + framing). Announcement + content-addressed fetch is deferred to the patch-pack milestone (noted in PLAN.md).
6. **Join sync of proposals/votes: out of scope, documented known limit.** Late joiners only see governance gossip sent after they connect (demo: host presses F9 after join). The M6 accepted-proposal ledger owns persistent governance state and its sync.
7. **Keyboard-only panel, cursor grab untouched.** `P` = open/back/close, Up/Down + Enter navigate, `Y`/`N` vote. Escape keeps its one meaning (release cursor). Zero changes to player.rs/interact.rs; gameplay input stays live (panel is an overlay). All chosen keys are unbound in-game.
8. **Ord derives** added to `ProposalId` and `PlayerId` (plain byte arrays) for `BTreeMap` keys.

## Step 1 — civora-governance: Vote

New `crates/civora-governance/src/vote.rs` (+ `mod vote;` and re-exports in `src/lib.rs`, crate doc update):

```rust
pub const VOTE_SIGN_DOMAIN: &[u8] = b"civora.vote.v1";
pub const VOTE_FORMAT_VERSION: u8 = 1;
pub const VOTE_BYTES: usize = 66;              // version(1) || proposal_id(32) || voter(32) || choice(1)
pub const MAX_SIGNED_VOTE_BYTES: usize = 130;  // + signature(64)

pub enum VoteChoice { No, Yes }                // No=0, Yes=1; from_byte rejects >1
pub struct Vote { pub proposal_id: ProposalId, pub voter: PlayerId, pub choice: VoteChoice }
// encode / decode / decode_exact per layout; reject unknown version/choice, truncation, trailing
pub struct SignedVote { pub vote: Vote, pub signature: [u8; 64] }
// sign (panics on voter mismatch) / encode (vote || signature, fixed size) / decode / decode_exact / verify
```

Derive additions: `PartialOrd, Ord` on `ProposalId` (`crates/civora-governance/src/proposal.rs` ~line 98) and on `PlayerId` (`crates/civora-identity/src/identity.rs` ~line 10).

Tests in new `crates/civora-governance/tests/vote.rs` (mirror tests/governance.rs): round-trip + exact 130-byte layout, truncation sweep, trailing byte, unknown version/choice, tamper (choice flip / sig flip / voter swap → `BadSignature`; garbage key → `BadAuthorKey`), cross-domain (action + proposal domains must not verify as votes and vice versa).

## Step 2 — civora-net: wire

- `crates/civora-net/Cargo.toml`: add `civora-governance = { path = "../civora-governance" }`.
- `crates/civora-net/src/wire.rs`: `CellRef::proposals_topic()` → `"civora/{PROTO_VERSION}/{realm}/{cell}/proposals"` (extend the topic-name test); `GossipMsg` gains `Proposal(SignedProposal)` tag 2 and `Vote(SignedVote)` tag 3, encode = tag byte + `signed.encode`, decode mirrors tag 0 (`rest.is_empty().then_some(..)`, structural only). Round-trip/truncation/trailing tests.

## Step 3 — civora-net: behaviour, commands, events, event loop

- `crates/civora-net/src/behaviour.rs` (~25-28): `const MAX_GOSSIP_BYTES: usize = 256 * 1024;` and build gossipsub with `gossipsub::ConfigBuilder::default().max_transmit_size(MAX_GOSSIP_BYTES).build()` (map its `Result` error like the existing string errors). Doc-comment the rationale.
- `crates/civora-net/src/lib.rs`: `NetCommand::{PublishProposal(SignedProposal), PublishVote(SignedVote)}` (doc: gossipsub doesn't loop back — insert into your own store too) and `NetEvent::{RemoteProposal, RemoteVote}` (doc: structurally decoded; verify/validate before trusting; delivered even while joining).
- `crates/civora-net/src/event_loop.rs`: `EventLoop` holds `proposals_topic` (next to `actions_topic` ~:120); add to subscribe array (~:52-60); `on_command` arms publish `GossipMsg::Proposal/Vote` on it; `on_gossip` (~:277) emits `RemoteProposal`/`RemoteVote` with no `self.live` gate.

## Step 4 — net integration test

Extend `crates/civora-net/tests/sync.rs` with `proposals_and_votes_gossip()` reusing `TestNode`/`wait_for`: host+joiner join, gossip mesh settles; host publishes a **large** proposal (1024 asset + 1024 wasm + 1024 migration cids, ~96+ KiB — pins the max_transmit_size raise); joiner receives it, `verify()`+`validate()` pass, id matches; joiner publishes `SignedVote{Yes}`; host receives it, `verify()` passes, fields match.

## Step 5 — client: ProposalStore + VotingPlugin (new `voting.rs`)

`crates/civora-client/Cargo.toml`: add civora-governance dep. New `crates/civora-client/src/voting.rs`, `mod voting;` in main.rs, `VotingPlugin` after `HudPlugin`.

```rust
const MAX_PENDING_VOTES: usize = 1024;
pub struct ProposalEntry { pub signed: SignedProposal, pub votes: BTreeMap<PlayerId, VoteChoice> }

#[derive(Resource, Default)]
pub struct ProposalStore { proposals: BTreeMap<ProposalId, ProposalEntry>, pending_votes: Vec<SignedVote> }
// insert_proposal: verify+validate gate, idempotent, drains matching pending votes
// insert_vote: verify gate; known proposal -> tally replace; unknown -> pending (replace same (proposal,voter), capped)
// open_count / iter / get / tally(id) -> (yes, no)

#[derive(Resource, Default, PartialEq, Eq, Clone, Copy)]
pub enum VotingUi { #[default] Closed, List { cursor: usize }, Detail { id: ProposalId } }
```

Store unit tests as `#[cfg(test)] mod tests` in voting.rs: dedup, bad-signature/invalid-manifest rejection, pending vote attaches when proposal arrives, revote replaces, tally counts.

Systems (Update, `.chain()`, `run_if(in_state(AppState::InGame))`):
- `voting_input`: `P` toggles Closed↔List (Detail→List); Up/Down move cursor (clamped); Enter opens Detail of cursor row; `Y`/`N` in Detail sign a `Vote` with `LocalIdentity`, `store.insert_vote` (same gate), then `NetCommand::PublishVote` if `NetChannels` exists. Repeat presses flip the vote.
- `sync_voting_panel`: house spawn/despawn on `ui.is_changed()`. Panel root `(VotingPanelRoot, Node{ position_type: Absolute, top: px(64), centered, width: px(560), padding 12 }, BackgroundColor(srgba(0.10,0.10,0.15,0.92)), GlobalZIndex(5))` with child `(VotingPanelText, Text::default(), TextFont{ font_size: FontSize::Px(14.0), .. }, TextColor(WHITE))` — menu.rs colors, below menu z 10.
- `update_voting_panel_text`: rebuild with `writeln!` like `update_debug_text`, ASCII only.
  - List: `OPEN PROPOSALS (N)`; rows `"> " / "  "` cursor + `1. <id.short()> <kind> by <author.short()>  yes <y> / no <n>`; empty: `(none - F9 publishes a sample)`; footer `Up/Down select  Enter details  P close`.
  - Detail (unknown id falls back to list): full id hex, kind, author, git 40-hex, source/build/tests cid shorts, wasm/assets/migrations counts (+ governance rule cid if any), `activation epoch <n> (epochs not tracked yet)`, rollback line, `TALLY yes <y> / no <n> (<total> voter(s))`, `your vote: yes|no|none`, footer `Y vote yes  N vote no  P back`.

HUD count: in `crates/civora-client/src/hud.rs` `update_debug_text`, add `store: Res<voting::ProposalStore>` and `writeln!(text, "proposals: {} open (P)", store.open_count())`.

## Step 6 — client: net pump arms

`crates/civora-client/src/net.rs` `pump_net_events` (~:156): add `ResMut<ProposalStore>`; arms `RemoteProposal` → `store.insert_proposal` (info on ok, debug on drop), `RemoteVote` → `store.insert_vote` (debug on drop).

## Step 7 — debug publish path

`crates/civora-client/src/debug.rs`: `sample_proposal(author, n)` — kind `AssetPatch`, `asset_cids: vec![Cid::of(&n.to_le_bytes())]` (distinct per press), git hash from first 20 bytes of a labeled `Cid::of`, distinct labeled cids for source/build/tests, `activation_epoch: 1000 + n` (placeholder), snapshot rollback; passes `validate()`. Systems (Update, InGame): `publish_sample_proposal_on_f9` (F9 → sign, `store.insert_proposal(..).expect(..)`, `NetCommand::PublishProposal` if channels, bump `Local<u32>` counter) and `auto_publish_sample_proposal` (`CIVORA_TEST_PROPOSAL` env → one sample ~3 s after startup, same shape as `auto_screenshot`).

## Step 8 — PLAN.md + plan doc

- Check off build-order item 5 with plans-doc link + done date.
- `### Milestone 5: voting UI` status section: vote message + domain, proposals topic, tags 2/3, store gate, keybinds.
- Build notes: keybinds (P/arrows/Enter/Y/N, F9, `CIVORA_TEST_PROPOSAL`); open-proposal definition (no epochs/windows until M6); gossipsub 256 KiB rationale + announcement/fetch deferral; **known limit: no join sync of proposals/votes — late joiners see only subsequent gossip**; revote = latest wins.
- Save this plan under `plans/` (house convention).

## Implementation order

1. governance vote.rs + Ord derives + vote tests
2. net wire.rs (topic, tags 2/3) + tests
3. net behaviour/lib/event_loop
4. net integration test
5. client voting.rs (store + UI state) + unit tests (parallel with 2–4)
6. client net.rs pump arms
7. client UI systems + hud.rs line
8. client debug.rs F9 + env hook
9. verification + PLAN.md/plans doc

## Verification

- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` (vote tests, wire tests, store unit tests, both net integration tests — the new one pins the gossip size raise with a >64 KiB proposal).
- Two-instance manual check (build-notes recipe with separate `--key-file`s): F9 on host → both HUDs show `proposals: 1 open (P)`; `P` on joiner → list; Enter → detail; `Y` on joiner → host's detail tally shows `yes 1`; flip votes and watch the tally move.
- Scripted screenshot: `CIVORA_TEST_PROPOSAL=1 CIVORA_SCREENSHOT=<path> CIVORA_SCREENSHOT_DELAY=6`.

## Known accepted limits (stated in PLAN.md)

- No epochs/voting windows/finality — every verified proposal stays open (M6).
- No join sync of governance gossip (M6 ledger owns persistence).
- Left-click still edits blocks while the panel is open (cursor stays grabbed by design; panel is keyboard-only).
