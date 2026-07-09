# Milestone 6: Accepted proposal ledger

## Context

PLAN.md build-order item 6, after Milestone 5 shipped proposal/vote gossip and a display-only tally. This milestone makes votes mean something: wall-clock epochs give `activation_epoch` real meaning (the voting window is open until it); at window close any peer whose local tally passes quorum assembles a **self-contained `FinalityCertificate`** (PLAN.md shape) and gossips it; accepted proposals enter an **append-only, disk-persisted ledger** ("Local Data Layer — accepted proposal ledger"); join sync closes the documented M5 limit (late joiners saw no governance state). **No patch application** — acceptance = ledger entry + UI status until the content-addressed patch packs milestone.

User-confirmed decisions: wall-clock epochs (`epoch = unix_secs / EPOCH_SECS`, no sync protocol); self-contained certificates (certifier snapshots its connected-peer roster incl. itself; verifiers check internal consistency only — a malicious certifier claiming a tiny roster is a documented alpha limit); the ledger records **accepted proposals only**.

## Key decisions

1. **Epochs are pure arithmetic**: `EPOCH_SECS = 30`; `epoch_at(unix_secs, epoch_secs)` takes the divisor as a parameter — that *is* the test seam (no trait, no mock). Client `EpochClock` resource reads `SystemTime::now()`, honors a `CIVORA_EPOCH_SECS` dev override (must match on all instances; certificate verification is clock-free, so mismatch only skews UX timing, never validity).
2. **Certificate stores the roster, derives the root** (the `ProposalId` never-stored-always-derived precedent): `eligible_roster: Vec<PlayerId>` (strictly ascending, non-empty, cap 1024) embedded; `eligible_roster_root()` = SHA-256 over `civora.roster-root.v1 || ids` is a method, so PLAN.md's field exists without a redundancy that could disagree.
3. **Vote lists are `(voter, sig)` pairs**, not full `SignedVote`s: `proposal_id` and `choice` are reconstructed at verify time from the certificate itself, so an embedded vote structurally cannot reference the wrong proposal or sit in the wrong list; 96 bytes/entry instead of 130. Requires exposing `vote::signing_payload` as `pub(crate)`.
4. **The certificate is certifier-signed** under new domain `civora.certificate.v1` (`SignedCertificate { certifier, certificate, signature }`, author-alongside like `SignedAction`); the roster claim is the trust-sensitive part, so it must be attributable. `verify` also requires `certifier ∈ eligible_roster`.
5. **`accepted_epoch` semantics**: assemble on the first tick where `now_epoch >= activation_epoch`; verification requires `accepted_epoch >= proposal.activation_epoch` (votes flip until close → no early certificates; `>=` not `==` lets a peer offline at close certify later from stored ballots). Votes carry no timestamps, so this is the only verifiable epoch check.
6. **Quorum v1** (integer math in civora-governance, shared by client and tests): ballots cast ≥ `MIN_QUORUM_BALLOTS = 1`; `Economy`/`Governance` need `yes * 3 > roster * 2` (supermajority); all other kinds `yes * 2 > roster` (majority); `Kernel` never passes. The PLAN.md table's automated-tests / sandbox-validation requirements defer to the patch-pack/wasm milestones (documented). Min quorum 1 means offline solo play self-accepts (roster = self) — deliberate, good for demos.
7. **`governance_rule_version`** starts at `GENESIS_RULE_VERSION: u32 = 1`; the ledger assigns numbering as `GovernanceChange`'s doc promised: `Ledger::rule_version() = 1 + accepted Governance-kind entries`; `append` rejects a certificate whose version ≠ current. Rule *semantics* don't change until governance-rule patching.
8. **Everyone certifies, the ledger dedups**: every peer whose roster-filtered tally passes quorum at close assembles, appends, and gossips (`GossipMsg::Certificate` tag 4). Certificates for one proposal may differ byte-wise (different rosters); **first valid certificate per `ProposalId` wins** per ledger (`append → Ok(false)` on dup). The accepted *set* converges; the bytes need not (documented).
9. **Ledger persistence in civora-governance** (`ledger.rs`, plain `std::fs` like `keyfile.rs`): magic `CIVLGR1\n` then self-delimiting `SignedProposal || SignedCertificate` entries; **whole-file rewrite via temp file + atomic rename** on every append (alpha ledgers are tiny; eliminates the truncated-tail case so load is strictly rejecting). Load rebuilds through `Ledger::append` — full re-verification of every signature.
10. **Ledger path mirrors the key file**: default `<config dir>/civora/genesis-0.ledger`, overridden by `--ledger-file` / `CIVORA_LEDGER_FILE` (two instances on one machine need distinct ledgers, same as keys).
11. **Join sync extends `SyncResponse::Accept`** (no second protocol): ledger entries + open proposals + their votes ride the existing sync response; **`PROTO_VERSION` bumps to 2** (breaking wire change — the version field exists for this; topics become `civora/2/...`). The event loop re-emits the payload as ordinary `RemoteProposal`/`RemoteCertificate`/`RemoteVote` events after `WorldSync`, so the client re-verifies everything through its existing gates. Merge = joiner appends unseen entries by `ProposalId` onto its persisted ledger; the host does not pull back (documented one-way limit).
12. **`ProposalEntry.votes` becomes `BTreeMap<PlayerId, SignedVote>`** (was `VoteChoice`): certification needs the signatures. The certificate freezes the certifier's latest-received ballots at close — M5's promised "binding votes to an ordering". Votes for closed proposals are rejected at the store gate; Y/N keys go inert after close.
13. **Proposal status is stored, not derived**: `Open | Accepted | Rejected | Expired` on the entry; the window-close system sets Rejected (ballots cast, quorum failed) / Expired (zero ballots); any valid certificate flips to Accepted, including over a local Rejected (the certifier's roster differed). HUD count = Open only.

## Step 1 — governance: epoch.rs

New `crates/civora-governance/src/epoch.rs` (+ `mod`/re-exports in `src/lib.rs`; update the crate doc that defers finality/ledger):

```rust
pub const EPOCH_SECS: u64 = 30;
pub fn epoch_at(unix_secs: u64, epoch_secs: u64) -> u64  // unix_secs / max(epoch_secs, 1)
```

Inline `#[cfg(test)]`: boundaries, zero-divisor guard.

## Step 2 — governance: certificate.rs

New `crates/civora-governance/src/certificate.rs`:

```rust
pub const CERT_SIGN_DOMAIN: &[u8] = b"civora.certificate.v1";
pub const ROSTER_ROOT_DOMAIN: &[u8] = b"civora.roster-root.v1";
pub const CERT_FORMAT_VERSION: u8 = 1;
pub const MAX_ROSTER_PLAYERS: usize = 1024;
pub const MAX_CERTIFICATE_BYTES: usize = 144 * 1024; // worst case ~131 KiB + headroom
pub const GENESIS_RULE_VERSION: u32 = 1;
pub const MIN_QUORUM_BALLOTS: usize = 1;

pub enum QuorumResult { Accepted } // byte 1; decoders reject others (rejection certs don't exist — ledger is accepted-only)

pub struct FinalityCertificate {
    pub proposal_id: ProposalId,
    pub governance_rule_version: u32,
    pub accepted_epoch: u64,
    pub quorum_result: QuorumResult,
    pub eligible_roster: Vec<PlayerId>,        // strictly ascending, non-empty
    pub yes_votes: Vec<(PlayerId, [u8; 64])>,  // strictly ascending by voter
    pub no_votes: Vec<(PlayerId, [u8; 64])>,   // strictly ascending by voter
}
// encode: version || proposal_id(32) || rule_version(u32 LE) || accepted_epoch(u64 LE)
//   || quorum byte || n_roster(u16 LE)+ids || n_yes(u16 LE)+(voter||sig)* || n_no likewise
// decode/decode_exact: reject unknown version/quorum byte, truncation, trailing,
//   empty roster, lists over cap, non-ascending/duplicate lists
// eligible_roster_root() -> [u8; 32]

pub fn quorum_passes(kind: ProposalKind, roster: usize, yes: usize, no: usize) -> bool;

pub struct SignedCertificate { pub certifier: PlayerId, pub certificate: FinalityCertificate, pub signature: [u8; 64] }
// payload = CERT_SIGN_DOMAIN || certifier || certificate.encode()
// encode: cert_len(u32 LE) || cert || certifier(32) || sig(64); len over MAX_CERTIFICATE_BYTES rejected
// sign(identity, certificate) — panics on certifier mismatch (house style)
// certify(identity, &Proposal, roster: &[PlayerId], ballots: &BTreeMap<PlayerId, SignedVote>,
//         rule_version: u32, accepted_epoch: u64) -> Option<SignedCertificate>
//   filters ballots to roster, runs quorum_passes, assembles + signs; None if quorum fails
// verify(&self, proposal: &Proposal, rule_version: u32) -> Result<(), CertificateError>
```

`verify` checks in order: `proposal.id() == proposal_id`; certifier signature; `certifier ∈ roster`; every yes/no voter ∈ roster; yes/no sets disjoint; each pair verifies as a `SignedVote` over the reconstructed `Vote { proposal_id, voter, choice }`; `quorum_passes`; `accepted_epoch >= proposal.activation_epoch`; `governance_rule_version == rule_version`. Distinct `CertificateError` variants per rejection.

Tests in new `crates/civora-governance/tests/certificate.rs` (mirror tests/governance.rs): round-trips; truncation sweep + trailing + unknown version/quorum byte; caps + non-canonical lists; full verify rejection matrix (proposal mismatch, certifier outside roster, tampered certifier/vote sigs, non-roster voter, voter in both lists, early epoch, wrong rule version); cross-domain (action/proposal/vote sigs never verify as certificates and vice versa); table-driven `quorum_passes` (roster 1/2/3/4/5, exactly-half, supermajority boundary `yes*3 == roster*2`, zero ballots); **golden vector** pinning `eligible_roster_root()` hex and the SHA-256 of a fixture `SignedCertificate` encoding.

## Step 3 — governance: ledger.rs

New `crates/civora-governance/src/ledger.rs` (+ `tempfile` dev-dependency):

```rust
pub const LEDGER_MAGIC: &[u8; 8] = b"CIVLGR1\n";
pub struct LedgerEntry { pub proposal: SignedProposal, pub certificate: SignedCertificate }
#[derive(Default)]
pub struct Ledger { entries: Vec<LedgerEntry>, ids: BTreeSet<ProposalId> }

impl Ledger {
    pub fn append(&mut self, entry: LedgerEntry) -> Result<bool, LedgerError>
    // verify-on-append (ActionLog template): proposal.verify()+validate(),
    // certificate.verify(&proposal, self.rule_version()); dedup by id -> Ok(false)
    pub fn contains / get / entries / len / is_empty
    pub fn rule_version(&self) -> u32  // GENESIS_RULE_VERSION + accepted Governance entries
    pub fn load(path: &Path) -> Result<Ledger, LedgerFileError>  // missing file => empty;
    //   strict decode (bad magic/truncation/trailing => Err); rebuild through append()
    pub fn save(&self, path: &Path) -> Result<(), LedgerFileError> // temp + atomic rename
}
```

Tests in new `crates/civora-governance/tests/ledger.rs`: happy path; dup → `Ok(false)`; tampered proposal/certificate rejected; rule-version increments on an accepted Governance entry + wrong-version cert rejected; save/load round-trip (tempfile); bad magic / truncated tail / trailing garbage / flipped byte → load error; missing file → empty.

## Step 4 — net: wire.rs

- `PROTO_VERSION: u32 = 2` (doc: governance join sync is a breaking response change).
- `GossipMsg::Certificate(Box<SignedCertificate>)` tag 4, mirroring tag 2 (structural decode only; ~132 KiB worst case fits `MAX_GOSSIP_BYTES` 256 KiB — extend that comment in behaviour.rs).
- `SyncResponse::Accept` gains `ledger: Vec<(SignedProposal, SignedCertificate)>`, `open_proposals: Vec<SignedProposal>`, `open_votes: Vec<SignedVote>` — each `count (u32 LE) || items` via the types' self-delimiting encodes, after `chunks`. Decode caps: `MAX_SYNC_LEDGER_ENTRIES = 64`, `MAX_SYNC_OPEN_PROPOSALS = 64`, `MAX_SYNC_VOTES = 8192` (worst-case entry ~324 KiB → governance payload ≤ ~34 MiB, safely under the 64 MiB `MAX_RESPONSE_BYTES` alongside the world; larger ledgers wait for announce-then-fetch in the patch-pack milestone — documented).
- Tests: tag-4 round-trip + truncation/trailing; extended `Accept` round-trip with populated payload; over-cap counts rejected.

## Step 5 — net: lib.rs + event_loop.rs

- `lib.rs`: `Snapshot` gains the three governance fields; `NetCommand::PublishCertificate(Box<SignedCertificate>)`; `NetEvent::RemoteCertificate(Box<SignedCertificate>)` (doc: structurally decoded, verify via the ledger gate; delivered even while joining).
- `event_loop.rs`: publish arm on the proposals topic; `on_gossip` tag-4 arm, no `live` gate; `ProvideSnapshot` builds the extended `Accept`; join response — after `WorldSync`, re-emit the governance payload in dependency order: per ledger entry `RemoteProposal` then `RemoteCertificate`, then open `RemoteProposal`s, then `RemoteVote`s. No verification in the net layer (house split).

## Step 6 — net integration tests

Extend `crates/civora-net/tests/sync.rs` (reuse `TestNode`/`wait_for`; give `TestNode` a `Ledger`). **No sleeps**: the net layer has no epoch logic — tests play the client's certifier role with `activation_epoch = current epoch` (window already closed).

- `certificate_gossip_reaches_both_ledgers`: host publishes a proposal; both nodes gossip yes ballots; host runs `certify` with roster `[host, joiner]`, appends, publishes tag 4; joiner receives `RemoteCertificate`, appends via its ledger gate; assert both `contains(id)` and a re-published dup yields `Ok(false)`.
- `join_syncs_governance_state`: host pre-loads one accepted ledger entry + one open proposal with a vote; joiner joins; after `WorldSync`, collect the re-emitted events, rebuild joiner store/ledger through the gates; assert the accepted entry, the open proposal, and the ballot all landed.

## Step 7 — client: EpochClock, LedgerStore, CLI

- `cli.rs`: `--ledger-file PATH`; `CliArgs.ledger_file: Option<PathBuf>`.
- New `crates/civora-client/src/ledger.rs` (`mod ledger;` in main.rs):

```rust
pub const EPOCH_SECS_ENV: &str = "CIVORA_EPOCH_SECS"; // dev knob, set identically on all peers
pub const LEDGER_FILE_ENV: &str = "CIVORA_LEDGER_FILE";

#[derive(Resource)] pub struct EpochClock { pub epoch_secs: u64 } // env override, default EPOCH_SECS
impl EpochClock { pub fn now_unix(&self) -> u64; pub fn now_epoch(&self) -> u64; }

#[derive(Resource)] pub struct LedgerStore { pub ledger: Ledger, pub path: PathBuf }
impl LedgerStore { pub fn append_and_save(&mut self, entry: LedgerEntry) -> Result<bool, ...> }
pub fn ledger_path(overridden: Option<PathBuf>) -> Result<PathBuf, String> // mirrors identity::key_path
```

- `main.rs`: resolve path + `Ledger::load` before the app starts (corrupt file = hard error naming the path, keyfile strictness); insert both resources.

## Step 8 — client: ProposalStore evolution (voting.rs)

- `ProposalEntry { signed, votes: BTreeMap<PlayerId, SignedVote>, status: ProposalStatus }`; `enum ProposalStatus { Open, Accepted, Rejected, Expired }`.
- `insert_proposal(signed, now_epoch)`: gates unchanged; initial status `Open` (evaluator closes past-window entries next tick); `insert_accepted(entry)` for ledger-derived seeding.
- `insert_vote(signed, now_epoch)`: new `StoreError::VotingClosed` when the target's window is closed or status ≠ Open; pending-vote parking unchanged.
- `pending_certs: Vec<SignedCertificate>` (cap `MAX_PENDING_CERTS = 64`, replace-by-proposal-id), drained on proposal insert.
- `open_count()` counts `Open` only.
- Unit tests: vote-after-close rejected; status transitions (Open→Rejected on quorum fail, →Expired on zero ballots, Rejected→Accepted on late certificate); pending certificate attaches; existing tests updated for the `SignedVote` map.

## Step 9 — client: window evaluation + certificate handling + net arms

- New system `evaluate_voting_windows` in client `ledger.rs` (Update, InGame, early-out ~once per second via `Local` timer): for each `Open` entry with `now_epoch >= activation_epoch`: skip if `ledger.contains(id)`; roster = `PeerRoster` (crates/civora-client/src/net.rs:150) ids + self, sorted/deduped; `SignedCertificate::certify(...)` with `ledger.rule_version()`, `accepted_epoch = now_epoch`; `Some` → `append_and_save`, status Accepted, `NetCommand::PublishCertificate` (if channels); `None` → Rejected (ballots > 0) / Expired (0). Offline solo: roster = self, own yes vote accepts.
- Shared helper `apply_certificate(store, ledger, cert)`: find proposal (store, else park), build `LedgerEntry`, `append_and_save` (that *is* the verification), on `Ok(_)` set status Accepted.
- `net.rs pump_net_events`: `RemoteCertificate` arm → `apply_certificate` (info on new accept, debug on dup/reject); `SnapshotRequested` extends `Snapshot` with ledger entries + `Open` proposals and their votes (truncate to wire caps with a warn); `RemoteVote` passes `now_epoch`.
- Startup seeding: `OnEnter(AppState::InGame)` inserts every persisted ledger entry's proposal as Accepted so history is visible.
- Ordering: `pump_net_events` (FixedUpdate) before `evaluate_voting_windows` (Update); even if the evaluator wins a race and marks Rejected, a certificate later overrides to Accepted.

## Step 10 — client: UI + debug

- `hud.rs`: `proposals: N open (P)` now counts Open only (shape unchanged).
- `voting.rs` UI: list rows gain `[open]/[accepted]/[rejected]/[expired]`; detail replaces `activation epoch N (epochs not tracked yet)` with `voting closes in Ns (epoch A, now E)` while open, else `status: ...`; accepted detail adds certificate lines (certifier short id, roster size, `yes Y / no N of R`, accepted epoch, rule version, roster root short hex). Y/N inert + footer `voting closed` when status ≠ Open.
- `debug.rs`: `sample_proposal(author, n, now_epoch)` sets `activation_epoch = now_epoch + DEMO_ACTIVATION_EPOCHS` (`= 3` → 60–90 s at 30 s epochs; seconds with `CIVORA_EPOCH_SECS=2`); F9 + auto-publish systems take `Res<EpochClock>`; **`CIVORA_TEST_PROPOSAL=1` also auto-votes yes** on its own sample so the scripted solo path completes unattended.

## Step 11 — PLAN.md + plans doc

- Check off build-order item 6 (plans-doc link + done date).
- `### Milestone 6: accepted proposal ledger` status section: certificate shape + domains, quorum rules, ledger file + atomic rewrite, join-sync mechanism, `PROTO_VERSION 2`.
- Build notes: epoch length + `CIVORA_EPOCH_SECS` (must match across instances); certificate trust limits (certifier-claimed roster, first-valid-wins, byte-divergent certs converge on the set); ledger location + `--ledger-file`/`CIVORA_LEDGER_FILE` (update the two-instance recipe); F9 window timing; deferred: patch application, real eligibility/anti-Sybil, rejection certificates, bidirectional ledger reconciliation/forking, announce-then-fetch.
- Save this plan under `plans/` (house convention).

## Implementation order

1. governance epoch.rs
2. governance certificate.rs + tests + golden vector
3. governance ledger.rs + persistence + tests
4. net wire.rs (tag 4, Accept extension, PROTO 2) + tests
5. net lib.rs/event_loop.rs
6. net integration tests
7. client cli/main/ledger.rs
8. client voting.rs store evolution + unit tests (parallel with 4–6)
9. client evaluator + pump arms + snapshot answer + seeding
10. client UI + debug.rs
11. verification + PLAN.md/plans doc

## Verification

- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.
- Two-instance manual demo (build-notes recipe + ledger files):
  ```
  CIVORA_PASSPHRASE=a cargo run -p civora-client -- --host --key-file /tmp/civ-a.key --ledger-file /tmp/civ-a.ledger
  CIVORA_PASSPHRASE=b cargo run -p civora-client -- --join /ip4/127.0.0.1/tcp/PORT/p2p/PEERID --key-file /tmp/civ-b.key --ledger-file /tmp/civ-b.ledger
  ```
  F9 on host → both HUDs `1 open`; detail shows a live countdown; both press Y; at close both flip to `[accepted]` with certificate info, HUD drops to `0 open`. Restart the joiner and rejoin → the accepted entry is present immediately (persisted + join-synced). Negative: vote No on one side with 2 peers → `rejected`; nobody votes → `expired`.
- Scripted screenshot: `CIVORA_EPOCH_SECS=2 CIVORA_TEST_PROPOSAL=1 CIVORA_SCREENSHOT=<path> CIVORA_SCREENSHOT_DELAY=12` (sample publishes ~3 s in, auto-votes yes, window closes ~6–8 s later, screenshot shows `[accepted]`).

## Known accepted limits (state in PLAN.md)

- A malicious certifier can claim a tiny roster; verification is internal-consistency only (real eligibility/anti-Sybil deferred).
- Certificates for one proposal may differ byte-wise across ledgers; the accepted set converges, the bytes need not.
- Join sync is joiner-pulls only; conflicting Governance entries across forked ledgers are a forking-milestone problem.
- Nothing is applied — acceptance is a ledger entry and UI status until content-addressed patch packs.
