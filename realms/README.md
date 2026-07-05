<!--
SPDX-FileCopyrightText: 2026 The Civora Authors
SPDX-License-Identifier: CC-BY-SA-4.0
-->

# Civora Realms

Realms are the worlds of Civora. Each realm is scaffolded here.

Realms mix **code** and **assets**, which carry different licenses:

- Realm **code** (systems, rules, Wasm module sources): `AGPL-3.0-or-later`,
  the same copyleft as the core.
- Realm **assets** (models, textures, audio, voxel packs): `CC-BY-SA-4.0`.

See [../COPYRIGHT.md](../COPYRIGHT.md) for the authoritative mapping.

## Layout

- `genesis-realm/` — the first playable shared world. Code is
  `AGPL-3.0-or-later`; everything under `genesis-realm/assets/` is
  `CC-BY-SA-4.0`.
- `templates/` — starting points for new realms, offered under
  `Apache-2.0 OR CC-BY-4.0` so they can be copied and modified freely.
