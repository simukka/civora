# Milestone 8: Wasm plugin ABI

## Context

PLAN.md build-order item 8, after M6 (accepted proposal ledger, plans/accepted-proposal-ledger.md) and M7 (content-addressed patch packs, plans/patch-packs.md) — **implement both first**; this plan builds on M6's `Ledger`/`LedgerStore` and M7's `BlobStore`/`ContentStore`/`PackTracker`. Today an accepted `GameplayCode` proposal's wasm blobs would land hash-verified on disk and stop there. This milestone gives them meaning: a new **`civora-kernel`** crate (the Reality Kernel's "Wasm sandbox" layer) embeds wasmtime behind a hand-rolled core-wasm ABI, and every peer **dry-runs** accepted modules against a fixed scratch world, displaying fuel used and the resulting world content-hash. Identical hash + fuel on every peer = the determinism demo. **The live shared world is never touched** — live mutation is M10, asset application is M9.

User-confirmed decisions: **wasmtime + hand-rolled core-wasm ABI** (explicit exports/imports, linear-memory buffers, canonical LE encoding, golden tests; no WIT/Component Model/wit-bindgen this milestone — namespaced so a component layer can wrap ABI v1 later); **sandboxed dry-run scope** (scratch world only; fuel + hash in the UI); **module bytes from the blob store** (accepted proposal → M7 pack fetch → kernel loads `wasm_module_cids` from `ContentStore`).

## Key decisions

1. **Crate name `civora-kernel`** — PLAN.md's Reality Kernel layer lists "Wasm sandbox" first, and M10's patch loader gets a home. Crate doc scope-limits honestly: sandbox + deterministic plugin executor; signature gates stay in `civora-identity`/client (the existing "kernel gate"). Depends on `civora-sim` (path) + wasmtime only.
2. **`wasmtime = "36"` (LTS line), minimal features**: `{ version = "36", default-features = false, features = ["runtime", "cranelift", "std"] }`; dev-deps add the `wat` feature + `wat = "1"` so tests compile inline WAT while the lib build stays minimal. Registry head is 46.0.1 with monthly major bumps; 36.x is the actively-patched LTS (MSRV 1.86, under the Bevy-driven 1.95 floor). A kernel should be boring: security patches without API churn. Excluded: async, cache (kernel does no I/O), gc, threads, component-model, parallel-compilation, pooling-allocator, profiling/coredump/debug features. Tier-1 on all four CI targets. If the minimal set fails anywhere, caret-46 is a one-line fallback — nothing here depends on post-36 API.
3. **ABI v1 = three guest exports + two host imports** (exact table below), everything namespaced `civora`. Entry point `civora_run(tick: i64) -> i32` — dry-runs pass `tick = 0`; M10 reuses the identical signature for live epoch ticks. `civora_abi_version` is a function (trivial for `#[no_mangle] extern "C"` Rust guests), checked by execution under a small fuel budget. **No allocator export in v1** — the host never writes variable-length data into guest memory (host→guest via return values, guest→host via ptr/len); `civora_alloc` is the documented first addition when M10 needs host-pushed payloads.
4. **`world_content_hash` is deliberately NOT an import**: host calls are unmetered by fuel and `content_hash()` is O(world) — unbounded wall time at near-zero fuel. ABI law, stated in the module doc: *every host import must be deterministic and O(1)-ish*. The hash lives in the host-side `RunReport`.
5. **Error signaling split**: world-state outcomes are **return codes** (`emit_action` → 0 applied, 1 guard no-op — a legitimate deterministic outcome modules may branch on); protocol violations are **traps** (OOB ptr/len, oversized, malformed `Action` bytes, over `MAX_ACTIONS_PER_RUN`). The host records `Option<AbiViolation>` in its state *before* trapping, so trap-reason mapping never parses wasmtime error strings.
6. **`emit_action` applies immediately via `tick::step(world, &[action])`** — one action per call, in emission order — so a subsequent `read_block` sees what the module built. `tick::step` stays the only mutation entrypoint even inside the sandbox; its guards are the validation. Dirty chunks accumulate into the report.
7. **Fixed scratch world**: `civora_kernel::dry_run_world() = VoxelWorld::flat(0)` (verified: stone chunk at y=−1, dirt y=0–2, grass surface y=3). Peers' live worlds legitimately differ mid-gossip/mid-fetch, so a live-world clone would make hash equality — the milestone's oracle — flaky by construction; a fixed world guarantees identical hashes, gives `read_block` real terrain, and `VoxelWorld` never needs `Clone`. Lives in the kernel so goldens and client share one definition. M10 runs against the real world at epoch boundaries.
8. **Setup failure vs execution outcome**: `run()` returns `Err(KernelError)` only pre-execution (validation, instantiation, ABI version). Once the module starts, everything — including fuel exhaustion — is `Ok(RunReport)` with `RunOutcome::Trapped { reason: TrapReason }` (our own enum, not wasmtime strings, so UI text is deterministic across peers).
9. **Dry-runs execute on Bevy's `AsyncComputeTaskPool`**, not inline: a module can legally be 16 MiB and adversarial cranelift compiles can take seconds — unacceptable frame hitch. `Engine` is Send+Sync; the client holds `Arc<Kernel>`, one task per accepted GameplayCode proposal, polled for completion.
10. **Sample module = checked-in WAT (source of truth) + checked-in assembled `.wasm`** embedded via `include_bytes!`. A kernel test with dev-dep `wat` pins `wat::parse_str(TOWER_WAT) == TOWER_WASM` (no drift); an `#[ignore]`d `regen_sample_wasm` test rewrites the binary after WAT edits. Keeps the client free of text-assembly machinery and makes `Cid::of(TOWER_WASM)` a stable golden. Rejected: build.rs assembly (first build script in repo, hides the voted-on bytes) and runtime assembly in debug.rs.
11. **F9 becomes the GameplayCode demo**: the M7 sample switches `kind` to `GameplayCode`, `put()`s `TOWER_WASM` as a sixth real blob, sets `wasm_module_cids = [Cid::of(TOWER_WASM)]`. The five M7 blobs stay (multi-blob pack UI still exercised). `CIVORA_TEST_PROPOSAL`/`CIVORA_TEST_VOTE` flows unchanged; M9 re-introduces an asset-only variant when it needs one.
12. **Fuel budget is a kernel parameter, never a kernel env read** (kernel does no I/O, no ambient config). The client resolves `CIVORA_PLUGIN_FUEL` (default `DEFAULT_FUEL_BUDGET = 100_000_000`) once into a resource. Peers should run the same value and the same wasmtime version (Cargo.lock pins it) for the fuel-equality demo.
13. **`PluginRuns` is session-local, not persisted** — dry-runs are cheap and deterministic; every session recomputes when packs complete (restart path included: M7 seeds packs complete from ledger + store). No new on-disk format.

## ABI v1

Constants in `crates/civora-kernel/src/lib.rs`:

```rust
pub const ABI_VERSION: i32 = 1;
pub const ABI_MODULE: &str = "civora";              // import module name
pub const EXPORT_MEMORY: &str = "memory";
pub const EXPORT_ABI_VERSION: &str = "civora_abi_version";
pub const EXPORT_RUN: &str = "civora_run";

/// = civora_governance::MAX_BLOB_BYTES (modules arrive as blobs); the client
/// static-asserts equality — the kernel cannot depend on civora-governance.
pub const MAX_MODULE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_GUEST_MEMORY_BYTES: usize = 16 * 1024 * 1024; // 256 wasm pages
pub const MAX_GUEST_TABLE_ELEMENTS: usize = 4096;
pub const MAX_WASM_STACK_BYTES: usize = 512 * 1024;
pub const MAX_ACTIONS_PER_RUN: u32 = 4096;
pub const MAX_EMIT_ACTION_BYTES: usize = 64;        // encoded Action is <= 14 today
pub const DEFAULT_FUEL_BUDGET: u64 = 100_000_000;
pub const VERSION_CHECK_FUEL: u64 = 100_000;        // instantiation + version call
```

**Guest exports** (all required; validation rejects wrong/missing types):

| Export | Type | Semantics |
|---|---|---|
| `memory` | `(memory N)`, declared min ≤ 256 pages | Linear memory; `emit_action` ptr/len resolve here. Growth past 256 pages denied by the store limiter (`memory.grow` returns −1, no trap). |
| `civora_abi_version` | `() -> i32` | Must return 1. Called once under `VERSION_CHECK_FUEL` before the entry point; other value → `WrongAbiVersion`, trap → `AbiVersionCheckFailed`. |
| `civora_run` | `(i64) -> i32` | Entry point; arg = tick number (0 in M8). Return = guest exit code, recorded verbatim (0 = success by convention). |

Also structural: **no start section** (instantiation must be inert so all execution is explicit and fueled) — checked by a ~20-line hand-rolled wasm section-header scan (magic `\0asm`, version 1, then `(id: u8, size: LEB128)` records; rejects LEB overflow/truncation). House style, no wasmparser dep.

**Host imports** (module `"civora"`; any import outside this exact set → `ForbiddenImport`, WASI included; memory/table/global imports forbidden):

| Import | Type | Semantics |
|---|---|---|
| `read_block` | `(i32,i32,i32) -> i32` | `world.get_block([x,y,z]).0` on the scratch world. Outside allocated chunks reads 0 (air). Total, deterministic, O(1); never traps. |
| `emit_action` | `(i32,i32) -> i32` | `(ptr,len)` into guest memory: bounds check, `len <= MAX_EMIT_ACTION_BYTES`, copy to a stack buffer, `Action::decode` (canonical — already rejects truncation/trailing/unknown tags), apply via `tick::step`. Returns 0 = applied, 1 = guard no-op. Traps (with recorded `AbiViolation`) on OOB ptr/len, over-length, malformed bytes, count over `MAX_ACTIONS_PER_RUN`. |

**Determinism config** (exact `wasmtime::Config` knobs):

```rust
config.consume_fuel(true);
config.cranelift_nan_canonicalization(true);  // canonical NaN bits
config.wasm_threads(false);                   // also off at the cargo-feature level
config.wasm_relaxed_simd(false);              // the one nondeterministic proposal; simd proper stays on
config.wasm_reference_types(false);
config.wasm_function_references(false);
config.wasm_gc(false);
config.wasm_multi_memory(false);
config.wasm_memory64(false);
config.wasm_tail_call(false);                 // deterministic, but keep v1 MVP-tight
config.max_wasm_stack(MAX_WASM_STACK_BYTES);
```

Not used: epoch interruption (fuel subsumes it), compilation cache, pooling allocator. Bulk memory stays on (Rust guests emit `memory.copy`/`fill`; deterministic). Per-store limits via `StoreLimitsBuilder::new().memory_size(MAX_GUEST_MEMORY_BYTES).table_elements(MAX_GUEST_TABLE_ELEMENTS).instances(1).memories(1).tables(1)`, installed with `store.limiter(...)`.

**Version-sensitive, verify on 36.0.x at implementation time**: `Store::set_fuel/get_fuel` signatures, `Trap::OutOfFuel` path, `Module::validate(&Engine, &[u8])`, `StoreLimitsBuilder` arg types, `Config::wasm_*` names, minimal-feature trap handling on windows/macos-arm64, `Store<T>`'s `'static` data bound (drives the `mem::take` design below; take/put-back works either way). Host closures return `wasmtime::Error` via wasmtime's re-export — never a direct anyhow dep.

## Step 1 — crate skeleton (`crates/civora-kernel/`)

- `Cargo.toml` (workspace stanza; deps per decision 2), root `Cargo.toml` members gains `"crates/civora-kernel"`.
- `src/lib.rs`: crate doc (what the sandbox is / is not; the O(1)-import ABI law; the no-WIT-yet note), constants, errors, and:

```rust
pub struct Kernel { engine: Engine, linker: Linker<HostState> }  // Linker built once, reused per store

impl Kernel {
    pub fn new() -> Result<Kernel, KernelError>;                   // deterministic config; client treats Err as startup hard error
    pub fn validate_module(&self, bytes: &[u8]) -> Result<(), KernelError>;  // static + structural only, no execution
    pub fn run(&self, module_bytes: &[u8], world: &mut VoxelWorld,
               fuel_budget: u64, tick: i64) -> Result<RunReport, KernelError>;
}

/// The fixed scratch world every peer dry-runs against — must be identical
/// on every peer or run hashes will not match. VoxelWorld::flat(0).
pub fn dry_run_world() -> VoxelWorld;

pub struct RunReport {
    pub outcome: RunOutcome,
    pub fuel_used: u64,              // budget - remaining
    pub actions_emitted: u32,        // decoded successfully
    pub actions_applied: u32,        // guards passed (world changed)
    pub dirty_chunks: Vec<ChunkPos>, // sorted, deduplicated
    pub world_hash: u64,             // content_hash() after the run — the cross-peer oracle
}
pub enum RunOutcome { Completed { exit_code: i32 }, Trapped { reason: TrapReason } }
pub enum TrapReason { OutOfFuel, AbiViolation(AbiViolation), GuestTrap }
pub enum AbiViolation { OutOfBoundsBuffer { ptr: u32, len: u32 },
    OversizedAction { len: u32 }, MalformedAction, TooManyActions }
pub enum KernelError { EngineSetup(String), ModuleTooLarge { len: usize }, NotWasm,
    HasStartSection, InvalidWasm(String), ForbiddenImport { module: String, name: String },
    MissingExport { name: &'static str }, WrongExportType { name: &'static str },
    GuestMemoryTooLarge { min_pages: u64 }, Instantiation(String),
    AbiVersionCheckFailed, WrongAbiVersion { got: i32 } }
```

Hand-rolled `Display` + `Error` impls (house style; `TrapReason`'s `Display` is UI text → stable and deterministic).

## Step 2 — `src/validate.rs`

`validate_module` + shared `validated_module(&self, bytes) -> Result<Module, KernelError>` so `run()` compiles once. Check order: size cap → magic/version + start-section scan → `Module::validate` under our config (threads/gc/relaxed-simd modules die here) → `Module::new` → import audit (`module.imports()`: exact module/name/`FuncType` against the ABI table) → export audit (`module.exports()`: memory named `memory`, min ≤ 256 pages; both functions with exact types).

## Step 3 — `src/host.rs`

```rust
pub(crate) struct HostState {
    world: VoxelWorld,          // mem::take'd from the caller for 'static store data
    dirty: BTreeSet<ChunkPos>,
    actions_emitted: u32, actions_applied: u32,
    violation: Option<AbiViolation>,
    limits: StoreLimits,
}
pub(crate) fn add_abi_imports(linker: &mut Linker<HostState>) -> Result<(), KernelError>;
```

`read_block` via `linker.func_wrap(ABI_MODULE, "read_block", |caller: Caller<'_, HostState>, x, y, z| ...)`. `emit_action`: resolve exported `Memory` from the caller, bounds/len checks, copy into `[u8; MAX_EMIT_ACTION_BYTES]`, `Action::decode`; on violation set `caller.data_mut().violation` **then** return `Err` to trap; on success cap-check + bump `actions_emitted`, one `tick::step`, extend `dirty`, bump `actions_applied` if chunks returned, return 0/1.

## Step 4 — `src/run.rs`

`run()` flow: `validated_module` → `HostState` with `mem::take(world)` → `Store::new` + limiter → `set_fuel(VERSION_CHECK_FUEL)` → `linker.instantiate` → typed `civora_abi_version()` call, require `Ok(1)` → `set_fuel(fuel_budget)` → typed `civora_run(tick)`:
- `Ok(code)` → `Completed { exit_code }`
- `Err(e)` → `Trapped`, reason resolved: recorded `violation` → `AbiViolation`; else `e.downcast_ref::<Trap>() == Some(Trap::OutOfFuel)` → `OutOfFuel`; else `GuestTrap`.

Then `fuel_used = budget − get_fuel()`, put the (mutated) world back into the caller's `&mut` on **every** path (no path may lose the world — including every early `Err`), build the report (`world_hash`, sorted `dirty_chunks`).

## Step 5 — sample module (`crates/civora-kernel/sample/` + `src/sample.rs`)

- `sample/tower.wat` (~50 lines, heavily commented — the human-readable artifact voters conceptually review). `civora_run(tick)` against `dry_run_world()`:
  1. `read_block(2,3,2)` must be GRASS(1) else exit 2 — proves reads.
  2. Write the 14-byte canonical `PlaceBlock` encoding at memory offset 0 (tag 0 `i32.store8`, LE coords at offsets 1/5/9, block byte at 13 — the WAT hardcodes the golden Action layout) and place a PLANK(4) column at `(2, y, 2)`, y = 4..=8 — 5 placements, each asserted 0.
  3. One deliberate guard no-op: place into dirt at `(2,2,2)`, assert `emit_action` returns 1 (else exit 3).
  4. One 13-byte `BreakBlock` (tag 1) of the grass at `(3,3,2)` — proves breaks.
  5. Return 0. Totals: emitted 7, applied 6. `tick` accepted and reserved (M10 varies behavior by it).
- `sample/tower.wasm`: assembled bytes, checked in.
- `src/sample.rs`: `pub const TOWER_WAT: &str = include_str!(...)`, `pub const TOWER_WASM: &[u8] = include_bytes!(...)`.
- Tests pin `wat::parse_str(TOWER_WAT) == TOWER_WASM`; `#[ignore]` `regen_sample_wasm` rewrites `tower.wasm` from the WAT (`cargo test -p civora-kernel regen_sample -- --ignored`, documented in the WAT header).

## Step 6 — kernel tests (`tests/kernel.rs` + inline)

Inline-WAT fixtures via the dev `wat` feature:

1. `golden_abi_vector`: a WAT module exercising every import/export → exact emitted/applied/exit_code/dirty_chunks + **pinned `world_hash` u64 literal**. Fuel asserted `> 0` but not pinned — fuel-per-instruction is a wasmtime implementation detail patch releases may change (comment says so); determinism is pinned by test 2.
2. `two_runs_are_identical`: same module twice → byte-identical `RunReport` including `fuel_used`.
3. `sample_module_golden`: `run(TOWER_WASM, &mut dry_run_world(), DEFAULT_FUEL_BUDGET, 0)` → `Completed { exit_code: 0 }`, emitted 7 / applied 6, pinned `world_hash`, pinned SHA-256 of `TOWER_WASM` (the demo blob's cid).
4. `fuel_exhaustion_traps_cleanly`: infinite loop → `Trapped { OutOfFuel }`, `fuel_used == budget`, report still produced, world returned.
5. `memory_growth_is_capped`: `memory.grow` past 256 pages returns −1 (no trap; exit code proves it); a `(memory 300)` module fails with `GuestMemoryTooLarge`.
6. `validate_rejects`: not-wasm → `NotWasm`; start section → `HasStartSection`; `"env"`/WASI import → `ForbiddenImport`; missing memory / missing `civora_run` → `MissingExport`; wrong signature → `WrongExportType`; over-cap → `ModuleTooLarge`; shared-memory (threads) module → `InvalidWasm`.
7. `wrong_abi_version`: returns 2 → `WrongAbiVersion { got: 2 }`; version fn traps → `AbiVersionCheckFailed`.
8. `emit_action_violations`: malformed / oversized / OOB / over `MAX_ACTIONS_PER_RUN` → `Trapped { AbiViolation(..) }` with the right variant; guard no-op returns 1 and doesn't bump `actions_applied`.
9. `read_block_matches_world`: flat-world values + air outside any chunk.
10. `wat_and_wasm_do_not_drift` + `regen_sample_wasm` (`#[ignore]`).

## Step 7 — client wiring (new `crates/civora-client/src/plugins.rs`, `mod plugins;` in main.rs)

```rust
pub const PLUGIN_FUEL_ENV: &str = "CIVORA_PLUGIN_FUEL";
const _: () = assert!(civora_kernel::MAX_MODULE_BYTES == civora_governance::MAX_BLOB_BYTES);

#[derive(Resource)] pub struct PluginKernel { pub kernel: Arc<Kernel>, pub fuel_budget: u64 }
#[derive(Resource, Default)] pub struct PluginRuns { runs: BTreeMap<ProposalId, Vec<ModuleRun>> }
pub struct ModuleRun { pub cid: Cid, pub result: ModuleRunResult }
pub enum ModuleRunResult { Ran(RunReport), Rejected(String) }   // Rejected = KernelError / missing-blob text
#[derive(Resource, Default)] struct PendingDryRuns(Vec<(ProposalId, Task<Vec<ModuleRun>>)>);
```

- `main.rs`: `Kernel::new()` before the app starts (hard error to stderr like the key/ledger/store paths); resolve `CIVORA_PLUGIN_FUEL` (parse failure = hard error naming the variable); insert the three resources.
- System `queue_dry_runs` (Update, InGame, after M7's pack systems): each `LedgerStore` entry with `kind == GameplayCode`, not yet in `PluginRuns`/`PendingDryRuns`, whose `PackTracker` pack is complete → read every `wasm_module_cids` blob from `ContentStore` (get miss/corrupt → `Rejected` immediately) → spawn one `AsyncComputeTaskPool` task (clone `Arc<Kernel>` + bytes) running each module against a fresh `dry_run_world()` with `tick = 0`.
- System `collect_dry_runs` (Update, InGame): poll with `future::poll_once`; on completion insert into `PluginRuns`, `info!("dry-run {}: hash {:016x} fuel {}", ...)` per module.

## Step 8 — client UI (`voting.rs`, `hud.rs`)

- Detail pane (accepted GameplayCode only), after M7's pack rows, cap `MAX_DETAIL_MODULE_ROWS = 8` (mirrors M7's blob-row cap). Per module: `wasm <cid8> hash <16-hex> fuel <n> exit <code>` | `wasm <cid8> trapped: <TrapReason>` | `wasm <cid8> rejected: <err>` | `wasm <cid8> dry-run pending...` — this is the determinism demo surface.
- `hud.rs`, when `PluginRuns` non-empty: `wasm dry-runs: N ok, M failed`, plus the most recent run's `wasm run <cid8>: hash <16-hex> fuel <n>` — the line two peers screenshot-compare.

## Step 9 — client demo path (`debug.rs`)

`sample_proposal` (M7 signature + store) per decision 11: `kind: GameplayCode`, sixth blob `put(TOWER_WASM)`, `wasm_module_cids: vec![Cid::of(TOWER_WASM)]` (single element = trivially ascending), `validate().expect(...)` still guards. `CIVORA_TEST_PROPOSAL`/`CIVORA_TEST_VOTE` otherwise untouched — acceptance triggers M7 fetch, pack completion triggers the dry-run; no new env on the happy path.

## Step 10 — PLAN.md + plans doc

- Check off build-order item 8 (`plans/wasm-plugin-abi.md` + done date); save this plan there.
- Status section: crate + LTS pin rationale, the ABI v1 table, determinism knobs, dry-run scope, fixed scratch world, `RunReport`.
- Build notes: `CIVORA_PLUGIN_FUEL` (default 100M; identical across peers for the fuel-equality demo), F9 now publishes GameplayCode with a runnable module, sample regen recipe, wasmtime upgrade policy (stay on the 36 LTS line; fuel counts/trap text may change across majors — bump deliberately; goldens don't pin fuel literals), demo recipe.

## Implementation order

1. Crate skeleton + workspace member + `Kernel::new` (verify the minimal feature set compiles first)
2. `validate.rs` + rejection tests
3. `host.rs` + `run.rs` + report/errors
4. Inline-WAT test suite
5. `tower.wat` + assembled `.wasm` + `dry_run_world` + sample goldens
6. Client `plugins.rs` + main.rs
7. Client UI
8. `debug.rs` GameplayCode sample
9. PLAN.md + plans doc + verification

(1–5 are pure kernel work; 6–8 depend on 5.)

## Verification

- `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`; watch the first CI run's debian-bookworm and windows release builds (new wasmtime dep).
- **Two-instance manual demo** (M6/M7 recipe, nothing new):
  ```
  CIVORA_PASSPHRASE=a CIVORA_EPOCH_SECS=5 cargo run -p civora-client -- --host \
    --key-file /tmp/civ-a.key --ledger-file /tmp/civ-a.ledger --store-dir /tmp/civ-a-store
  CIVORA_PASSPHRASE=b CIVORA_EPOCH_SECS=5 CIVORA_TEST_VOTE=1 cargo run -p civora-client -- \
    --join /ip4/127.0.0.1/tcp/PORT/p2p/PEERID \
    --key-file /tmp/civ-b.key --ledger-file /tmp/civ-b.ledger --store-dir /tmp/civ-b-store
  ```
  Host presses F9 + Y; joiner auto-votes; at window close both flip `[accepted]`, packs count to 6/6, and **both HUDs show the identical `wasm run <cid8>: hash <H> fuel <F>` line** — same hash, same fuel, two independent peers. Negatives: joiner restart re-runs from ledger + store, same hash; `CIVORA_PLUGIN_FUEL=1000` → detail pane shows `trapped: out of fuel` without touching the live world or session.
- **Scripted screenshot**: host `CIVORA_EPOCH_SECS=2 CIVORA_TEST_PROPOSAL=1 CIVORA_SCREENSHOT=/tmp/civ-m8-a.png CIVORA_SCREENSHOT_DELAY=25 … --host …`; joiner `CIVORA_EPOCH_SECS=2 CIVORA_TEST_VOTE=1 CIVORA_SCREENSHOT=/tmp/civ-m8-b.png CIVORA_SCREENSHOT_DELAY=25 … --join …` — both screenshots show the same `wasm run …` HUD line.

## Known accepted limits (state in PLAN.md)

- No Component Model / WIT — core-wasm ABI, namespaced so a component layer can wrap v1 later.
- Dry-run only: nothing touches the live world until M10; no persistent plugin state (fresh instance + scratch world per run); single entry point.
- No capability/permission system yet — the only capabilities *are* the two imports.
- Host imports are unmetered by fuel (mitigated by the O(1)-import law).
- `validate`/`run` compile the module: wall-time up to the 16 MiB cap is accepted (background thread hides it; the module was socially accepted first).
- Fuel/hash equality guaranteed only on identical wasmtime versions — Cargo.lock pins it (matches the reproducible-build goal).
- Cross-CPU determinism (x86_64 vs arm64) is wasmtime's designed guarantee under NaN canonicalization + disabled relaxed-SIMD/threads, but is asserted, not yet CI-verified across our four targets.
