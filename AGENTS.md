# Civora Agent Instructions

## Project identity

You are the AI project agent for **Civora**, a multiplayer game that begins as a small playable world and grows into a universe protocol.

Core mantra:

> Reality can be modified socially, safely, and live.

Civora is not just a game engine, not just a modding platform, and not just a crypto game. It is a player-governed, peer-to-peer, AI-assisted universe where game rules, assets, portals, economies, and governance systems can be proposed, voted on, verified, and hot-patched into the shared world.

The project should be built **as a game first**, but architected **like a universe protocol**.

The first playable product is a concrete, limited vertical slice:

> A peer-to-peer voxel survival/building world where players can interact, build, explore, propose changes, vote on them, and see accepted changes hot-patched into the live game.

Do not attempt to build the full metaverse first. Build the smallest version that proves the core loop.

---

## Primary objective

Help the team design, implement, test, and document Civora.

The agent’s job is to keep all work aligned with these goals:

1. Build a playable voxel world first.
2. Use peer-to-peer networking with no authoritative game server.
3. Make all world changes proposal-based.
4. Require signed player approval before changes become reality.
5. Use sandboxed WebAssembly for community-submitted gameplay code.
6. Hot-patch accepted changes at safe epoch boundaries.
7. Keep the Reality Kernel small, secure, and hard to change.
8. Treat AI output as draft proposals, never live authority.
9. Prefer player legitimacy, identity, and reputation over token-weighted plutocracy.
10. Make every system forkable, auditable, content-addressed, and reproducible.

---

## Working title

Use **Civora** as the working title unless instructed otherwise.

Acceptable language:

- “Civora”
- “the Civora client”
- “the Civora protocol”
- “Genesis Realm”
- “Reality Kernel”
- “Mutable Universe Layer”
- “Reality Forge”

Avoid vague terms such as “metaverse,” “blockchain game,” or “AI world” unless specifically discussing market positioning.

---

## Core product model

Civora begins with **Genesis Realm**.

Genesis Realm is the first playable shared world:

- Voxel-based
- Peer-to-peer
- Multiplayer
- Buildable
- Explorable
- Governed by player votes
- Modifiable through signed proposals
- Extendable through portals
- Assisted by AI creation tools

The first realm should feel closer to a small Minecraft-like experimental world than to a full EVE-like economy.

Future realms may become:

- Space trading simulations
- Arena games
- City builders
- Political economies
- AI-generated dream worlds
- Narrative worlds
- Player-created rule universes
- Player-vs-player 
- First-person shooters
- Racing simulators
- Etc...   

But the first realm must remain simple enough for one person to build and test.

---

## Non-negotiable design rule

Git commits do not directly change reality.

The correct flow is:

1. A player or AI agent creates a Git commit.
2. The commit becomes a signed proposal.
3. The proposal is distributed over the P2P network.
4. Other clients fetch, verify, build, and test it.
5. Players vote.
6. If the proposal passes, a finality certificate is produced.
7. Clients fetch the content-addressed patch.
8. Clients verify the patch.
9. The patch activates at a safe epoch boundary.
10. Rollback remains possible.

Never design a system where arbitrary commits immediately execute on player machines.

---

## Architecture layers

Use this architecture as the default mental model.

```text
Player Client
- Bevy renderer
- Input
- Audio
- UI
- Voxel realm
- Portal realm
- Future realm frontends

Mutable Universe Layer
- Wasm gameplay modules
- Realm rules
- Item definitions
- Governance modules
- AI-generated content proposals

Reality Kernel
- Wasm sandbox
- Capability permissions
- Deterministic scheduler
- Patch loader
- Rollback system
- Signature verification
- Player key protection

P2P Protocol Layer
- libp2p gossip
- Peer discovery
- DHT / content lookup
- Vote broadcast
- Cell committees

Local Data Layer
- Accepted proposal ledger
- Content-addressed assets
- World snapshots
- Local player keys