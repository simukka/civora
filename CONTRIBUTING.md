# Contributing to Civora

Thanks for helping build Civora. This project's guiding rule (see
[AGENTS.md](AGENTS.md)) is:

> Reality can be modified socially, safely, and live.

In practice that shapes how we accept changes: **Git commits do not directly
change reality — they become proposals.** Contributing here is the first half of
that flow.

## Ground rules

- Be respectful and assume good faith.
- Prefer small, reviewable changes.
- Keep the Reality Kernel small, deterministic, and secure. Security-sensitive
  changes get extra scrutiny (see [SECURITY.md](SECURITY.md)).
- Treat AI-generated output as a **draft proposal**, never as authority. Review
  it like any other patch.

## Licensing of contributions (inbound = outbound)

By contributing, you agree that your contribution is licensed under the license
that **already applies to the file or directory you are changing**, as recorded
in [COPYRIGHT.md](COPYRIGHT.md):

- Core software (`crates/**`, realm code): **AGPL-3.0-or-later**
- SDK (`sdk/**`): **Apache-2.0**
- Assets (`assets/official/**`, realm assets): **CC-BY-SA-4.0**
- Realm templates (`realms/templates/**`): **Apache-2.0 OR CC-BY-4.0**

You keep the copyright to your work. There is **no CLA and no copyright
assignment**. If you want to contribute something under different terms, say so
explicitly in the pull request and we will discuss before merging.

Do not contribute code or assets you do not have the right to license this way.
In particular, do not add third-party assets or trademarked material (including
other projects' logos) without a compatible license, and do not modify the
Civora brand assets (see [TRADEMARK.md](TRADEMARK.md)).

## Developer Certificate of Origin (DCO)

We use the [Developer Certificate of Origin](https://developercertificate.org/).
It is a simple statement that you wrote, or otherwise have the right to submit,
the code. Certify it by signing off every commit:

```sh
git commit -s -m "your message"
```

This appends a line to your commit message:

```
Signed-off-by: Your Name <you@example.com>
```

Use a real name and a reachable email. Commits without a sign-off may be asked
to amend before merge.

## SPDX headers on new files

Add an SPDX header to the top of every new text/source file so licensing travels
with the file. Use the comment syntax of the language.

Rust / C-style:

```rust
// SPDX-FileCopyrightText: 2026 The Civora Authors
// SPDX-License-Identifier: AGPL-3.0-or-later
```

Shell / TOML / YAML:

```sh
# SPDX-FileCopyrightText: 2026 The Civora Authors
# SPDX-License-Identifier: AGPL-3.0-or-later
```

Pick the identifier that matches the area you are in (`Apache-2.0` under `sdk/`,
`CC-BY-SA-4.0` for assets, and so on). For binary files (images, audio), either
add a matching `<filename>.license` sidecar file or rely on the glob rules in
[REUSE.toml](REUSE.toml). You can verify coverage with:

```sh
reuse lint
```

## Development workflow

1. Fork and branch from `main`.
2. Make your change, with tests where it makes sense.
3. Run the same checks CI runs:

   ```sh
   cargo fmt --check
   cargo clippy --workspace -- -D warnings
   cargo test --workspace
   ```

4. Sign off your commits (`-s`) and open a pull request describing the change
   and its rationale. For gameplay- or governance-affecting changes, describe
   the change as if writing the proposal it will eventually become: what it
   does, why, and how to roll it back.

## Building on Civora without contributing upstream

You do not have to upstream anything. You can build modules with the Apache-2.0
SDK and ship them under your own terms. If you run a modified Civora node for
other players, remember the core is AGPL-3.0-or-later: you must offer those
users the corresponding source.

Welcome aboard.
