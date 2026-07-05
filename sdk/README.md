<!--
SPDX-FileCopyrightText: 2026 The Civora Authors
SPDX-License-Identifier: Apache-2.0
-->

# Civora SDK

The Civora SDK is the permissive, third-party-friendly surface for building on
Civora: the plugin ABI, Rust helpers, and examples for writing sandboxed
WebAssembly gameplay modules.

**License:** everything under `sdk/` is licensed under the Apache License 2.0
(`SPDX-License-Identifier: Apache-2.0`). See
[../LICENSES/Apache-2.0.txt](../LICENSES/Apache-2.0.txt) and
[../COPYRIGHT.md](../COPYRIGHT.md).

Apache-2.0 is used here — rather than the AGPL that covers the core — so anyone
can build modules and tools without copyleft obligations.

## Layout

- `rust/` — Rust SDK crate(s) for authoring Wasm modules against the Civora
  component interfaces.
- `wit/` — WebAssembly Component Model (WIT) interface definitions: the stable
  plugin ABI (for example `spawn_entity`, `read_voxel`, `emit_event`,
  `cast_vote`, `open_portal`).
- `examples/` — example modules and integrations.

> Placeholder: these directories are scaffolding for the licensing structure.
> When code lands here, add an `Apache-2.0` SPDX header to each new file.

## Redistribution note

When publishing an SDK component standalone, ship a copy of
`LICENSES/Apache-2.0.txt` and the repository `NOTICE` file, as required by
Apache-2.0 Section 4.
