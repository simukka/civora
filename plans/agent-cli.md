# Proposal CLI for agents (`civora-cli`)

## Context

Civora's vision is "git commits become proposals; proposals become reality only after signed player approval." Milestone 5 shipped proposal/vote gossip and the in-game voting UI, but the only way to create a proposal is the F9 debug key inside the Bevy client — useless for automation. The user wants a CLI that AI/automation agents can drive to create real proposals: non-interactive, scriptable, machine-readable output. Confirmed scope: **create + publish** (signed proposal files, and gossiping them into a live session), **git-aware inputs with explicit-flag overrides**, **no vote subcommand** (that waits for the M6 ledger).

Constraints that shape the design (verified in code):

- No join sync of governance gossip (M5 known limit) — a published proposal only reaches peers online at publish time; the CLI must dial a live session.
- `event_loop::publish` (`crates/civora-net/src/event_loop.rs:203`) swallows gossipsub errors (`InsufficientPeers` at debug level) — a one-shot publisher gets **no delivery signal** today. Receiving stores are idempotent, so republishing is always safe.
- House rules: hand-rolled canonical codecs, **no serde** (so no JSON output; line-based `key=value` instead), no CLI-parser dependency (`crates/civora-client/src/cli.rs` precedent), ASCII-only.
- `civora-identity` deliberately contains no interactive I/O; the client's keyfile loader (`crates/civora-client/src/identity.rs`) prompts via rpassword with `CIVORA_PASSPHRASE` as the non-interactive path.
- Canonical encoding requirements: `wasm_module_cids`/`asset_cids` must be strictly ascending (set semantics — the CLI sorts + dedups), `migration_cids` keep caller order but reject duplicates (`Proposal::validate`), `ProposalKind::Kernel` is rejected, `kind == Governance ⟺ governance_change.is_some()`.

## Deliverable

New workspace crate `crates/civora-cli`, binary named `civora`:

```
civora propose  [flags]              build + validate + sign -> file, print id
civora publish  (--file P | [flags]) dial a session, gossip the proposal, confirm
civora inspect  --file P             decode + verify + validate, print all fields
```

Stdout is one `key=value` pair per line (e.g. `proposal_id=<64 hex>`); diagnostics go to stderr. Exit codes: 0 success, 1 failure, 2 usage error.

### `civora propose`

| Flag | Meaning | Default |
|---|---|---|
| `--kind K` | `asset-patch\|new-content\|gameplay-code\|economy\|governance` | required |
| `--repo PATH` | repo for git subcommands | `.` |
| `--commit HEX40` | proposed commit | `git rev-parse HEAD` in `--repo` |
| `--source-bundle V` | source bundle cid | hash of `git archive <commit>` stdout |
| `--build-manifest V` | build manifest cid | required |
| `--test-results V` | test results cid | required |
| `--wasm V` (repeat) | wasm module cids | empty |
| `--asset V` (repeat) | asset cids | empty |
| `--migration V` (repeat) | migration cids, execution order | empty |
| `--governance-rule V` | rule module cid (Governance kind only) | none |
| `--activation-epoch N` | placeholder until M6 | `1000` |
| `--rollback snapshot` / `--rollback-migration V` (repeat) | rollback plan | `snapshot` |
| `--out PATH` | output file for canonical `SignedProposal` bytes | `<id-short>.proposal` |
| `--key-file PATH` | identity key (also `CIVORA_KEY_FILE`) | OS config dir |

Every cid-valued `V` is either a **file path** (hashed with `Cid::of`, `crates/civora-governance/src/cid.rs`) or **`hex:<64 hex>`** for a precomputed cid. Git values come from `std::process::Command` (`git rev-parse`, `git archive`) — no git library dependency. `Proposal::validate()` runs before signing; sign with `SignedProposal::sign`. Prints `proposal_id=`, `author=`, `file=`, `bytes=`.

### `civora publish`

Accepts `--file PATH` (a `propose` output, decoded with `SignedProposal::decode_exact` + verify + validate) **or** the full `propose` flag set (inline create; also writes `--out` if given). Session flags: `--join MULTIADDR` (else mDNS discovery, like the client), `--wait SECS` overall deadline (default 30).

Flow: load identity → `civora_net::spawn` with `SessionMode::Join` → wait for `PeerConnected` → ~2 s mesh settle (same as `crates/civora-net/tests/sync.rs`) → send `NetCommand::PublishProposal(Box::new(signed))` → wait for the new publish-outcome event (below), retrying on failure until `--wait` expires. The `WorldSync`/`SnapshotRequested` events a joiner sees are ignored. Prints `proposal_id=` and `published=true|false`; exit code follows.

### Small `civora-net` change: a publish-outcome signal

In `crates/civora-net/src/event_loop.rs`, for the `PublishProposal`/`PublishVote` arms only (actions/beacons stay fire-and-forget), surface the gossipsub publish result as a new event:

```rust
/// Outcome of a PublishProposal/PublishVote command. `ok` means gossipsub
/// accepted the message for at least one mesh peer — republishing after a
/// failure is safe (receiving stores are idempotent).
NetEvent::GovernancePublished { ok: bool, reason: Option<String> },
```

`crates/civora-client/src/net.rs` `pump_net_events` gets one arm (debug-log failures). This turns the CLI's exit code into a real delivery signal instead of a sleep-and-hope.

### Identity loading

Small `keys.rs` in the CLI mirroring the client's `load_or_create` (`crates/civora-client/src/identity.rs`): `CIVORA_PASSPHRASE` first (the agent path), rpassword prompt as fallback, `--key-file`/`CIVORA_KEY_FILE` override, auto-create on first run. Not extracted into `civora-identity` — that crate's contract is "no interactive I/O". Deps: `dirs`, `rpassword` (both already in the workspace).

## Files

- `crates/civora-cli/Cargo.toml` — `[[bin]] name = "civora"`; deps: civora-governance, civora-identity, civora-net, dirs, rpassword.
- `crates/civora-cli/src/main.rs` — dispatch + exit codes; thin, logic lives in modules so tests can call it.
- `crates/civora-cli/src/args.rs` — hand-rolled parser in the `cli.rs` house style.
- `crates/civora-cli/src/manifest.rs` — cid-value resolution (path vs `hex:`), git helpers, list sort/dedup, `Proposal` assembly.
- `crates/civora-cli/src/keys.rs` — keyfile load-or-create.
- `crates/civora-cli/src/publish.rs` — join/settle/publish/confirm loop against a `NetHandle`.
- `crates/civora-net/src/event_loop.rs`, `src/lib.rs` — `GovernancePublished` event.
- `crates/civora-client/src/net.rs` — one new match arm.
- `Cargo.toml` (workspace members), `PLAN.md` (build-notes entry), new `AGENTS.md` at repo root documenting the agent workflow (create → publish → verify in HUD), plan saved under `plans/` per house convention.

## Tests

- Unit (`#[cfg(test)]` in args/manifest): flag parsing errors, `hex:` vs path resolution, set-list sorting + dedup, migration duplicate rejection, kind/governance-change consistency, assembled proposal passes `validate()`.
- Round trip (crate test): propose-to-file → `inspect` decodes, verifies, validates, same id.
- Integration (`crates/civora-cli/tests/publish.rs`): reuse the `TestNode` pattern from `crates/civora-net/tests/sync.rs` — spawn a host node, run the CLI's publish function against its listen addr, assert the host receives a verified `RemoteProposal` and the CLI saw `GovernancePublished { ok: true }`.

## Verification

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- End-to-end: `CIVORA_PASSPHRASE=a cargo run -p civora-client -- --host --key-file /tmp/civ-a.key` then `CIVORA_PASSPHRASE=b cargo run -p civora-cli -- publish --kind asset-patch --asset <file> --build-manifest <file> --test-results <file> --join <addr> --key-file /tmp/civ-b.key`; the client HUD shows `proposals: 1 open (P)` and `P` shows the manifest. Scripted variant with `CIVORA_SCREENSHOT` as in M5.

## Known accepted limits (documented in AGENTS.md / PLAN.md)

- A publish reaches only currently-online peers (M5 known limit; the M6 ledger owns persistence).
- The joiner-mode publish transfers a world snapshot it ignores — wasteful but harmless; a lighter "governance-only" session mode can come later.
- Cids hash whatever bytes the agent points at; nothing fetches or verifies artifact content until the patch-pack milestone.
