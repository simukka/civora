![image](assets/logo/logo_1024.png)

# Civora Copyright and Licensing

Civora is intentionally multi-licensed. This document is the authoritative,
human-readable map of which license applies to which part of the repository.
The full license texts live in [LICENSES/](LICENSES/), and the same mapping is
declared machine-readably in [REUSE.toml](REUSE.toml) following the
[REUSE specification](https://reuse.software).

If this document and `REUSE.toml` ever disagree, treat it as a bug and open an
issue — they are meant to describe the same thing.

## Copyright holders

Copyright © 2026 **The Civora Authors**.

"The Civora Authors" means the individuals and entities who have contributed to
this repository. Contributors retain copyright in their contributions and
license them to the project and its users under the applicable license for the
file they touched (inbound = outbound). There is **no copyright assignment and
no CLA**. See [CONTRIBUTING.md](CONTRIBUTING.md). Per-file authorship is
recorded in Git history and, where present, in SPDX `SPDX-FileCopyrightText`
headers.

## License map

| Path | License (SPDX) | Kind |
| ---- | -------------- | ---- |
| `crates/reality-kernel/**` | `AGPL-3.0-or-later` | Core software |
| `crates/civora-client/**` | `AGPL-3.0-or-later` | Core software |
| `crates/p2p-protocol/**` | `AGPL-3.0-or-later` | Core software |
| `crates/proposal-ledger/**` | `AGPL-3.0-or-later` | Core software |
| `crates/patch-verifier/**` | `AGPL-3.0-or-later` | Core software |
| `crates/civora-sim/**` | `AGPL-3.0-or-later` | Core software (current) |
| `crates/civora-identity/**` | `AGPL-3.0-or-later` | Core software (current) |
| `sdk/rust/**` | `Apache-2.0` | SDK |
| `sdk/wit/**` | `Apache-2.0` | SDK (plugin ABI) |
| `sdk/examples/**` | `Apache-2.0` | SDK |
| `realms/genesis-realm/**` (code) | `AGPL-3.0-or-later` | Realm code |
| `realms/genesis-realm/assets/**` | `CC-BY-SA-4.0` | Realm assets |
| `realms/templates/**` | `Apache-2.0 OR CC-BY-4.0` | Templates |
| `assets/official/**` | `CC-BY-SA-4.0` | Official assets |
| `assets/logo/**` | `LicenseRef-Civora-Trademark` | Brand / trademark |

### Current crates

The map above lists the **target** crate layout from the project plan. Today
the workspace ships three core crates — `civora-sim`, `civora-identity`, and
`civora-client` — all under `AGPL-3.0-or-later`. As the Reality Kernel, P2P
protocol, proposal ledger, and patch verifier are split into their own crates,
they inherit the same `AGPL-3.0-or-later` license. New core crates should set
`license.workspace = true` in their `Cargo.toml`.

### Root-level files

| File / pattern | License (SPDX) | Notes |
| -------------- | -------------- | ----- |
| `civora_logo.svg`, `logo_concept.png` | `LicenseRef-Civora-Trademark` | Brand assets, not freely relicensed |
| `concept*.png`, `civora-screenshot-*.png` | `CC-BY-SA-4.0` | Concept art & screenshots |
| `*.md`, `plans/**` | `CC-BY-SA-4.0` | Documentation & prose |
| `Cargo.toml`, `Cargo.lock`, `.github/**`, `.gitignore`, `REUSE.toml`, `LICENSE`, `NOTICE` | `AGPL-3.0-or-later` | Build & project tooling |
| `LICENSES/**` | (the license texts themselves) | Verbatim upstream texts |

## Why these licenses

- **AGPL-3.0-or-later for the core.** Civora is peer-to-peer with no
  authoritative server. Copyleft — specifically the AGPL network clause —
  ensures anyone who runs a modified node for other players must make the
  corresponding source available. This keeps the shared reality forkable,
  auditable, and impossible to quietly capture.
- **Apache-2.0 for the SDK.** Building modules, tools, and integrations should
  be frictionless. A permissive, patent-protective license lets third parties
  build on the plugin ABI (WIT) and Rust SDK without copyleft obligations.
- **CC-BY-SA-4.0 for official assets.** Art, audio, and world assets are shared
  under share-alike so improvements flow back and the official look stays open.
- **Apache-2.0 OR CC-BY-4.0 for templates.** Realm templates are starting
  points meant to be copied and modified freely, for both code and content,
  with attribution.
- **Trademark for the name and logo.** The Civora name and logo identify this
  project and its official builds. They are protected so forks can use the free
  code while users are not misled about origin. See [TRADEMARK.md](TRADEMARK.md).

## SPDX identifiers and REUSE

Every licensed area is expressed with an SPDX identifier. New source files
should carry an SPDX header, for example:

```
// SPDX-FileCopyrightText: 2026 The Civora Authors
// SPDX-License-Identifier: AGPL-3.0-or-later
```

Areas and binary/asset files without their own header are covered by
[REUSE.toml](REUSE.toml). You can check licensing coverage with `reuse lint`
(see <https://reuse.software>).

## Third-party code

Civora depends on third-party libraries (for example Bevy, Wasmtime, libp2p,
and ed25519-dalek). Those dependencies are fetched at build time and remain
under their own licenses; they are not relicensed by this repository. Notable
acknowledgements are listed in [NOTICE](NOTICE).
