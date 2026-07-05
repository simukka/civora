
# First milestone: 90-day vertical slice
The first 90 days should prove the thesis, not the whole universe. 
The win is not player count yet. The win is proving that reality can 
be modified socially, safely, and live.

## Target demo
Twelve to thirty-two players join a peer-to-peer voxel world.
One player uses AI to generate a new object.
All online players receive an in-game vote.
The majority approves.
Every client fetches the content-addressed patch, verifies signatures, 
loads the Wasm module, and the new object appears in the live world 
without restarting.

That is the magic moment.

## 90-day deliverables

| Area         | Deliverable                              |
| ------------ | ---------------------------------------- |
| Client       | Native Rust/Bevy app                     |
| World        | Small voxel map with building/mining     |
| Network      | P2P join, discovery, gossip              |
| Identity     | Player keypair and signed actions        |
| Governance   | Proposal, vote, finality certificate     |
| Patch system | Asset + Wasm hot patch at epoch boundary |
| AI           | Prompt-to-proposal prototype             |
| Portal       | One portal to a tiny second test realm   |
| Build        | Reproducible proposal pack               |
| Security     | Wasm permissions, fuel limits, rollback  |

## Six-month target
| System     | Goal                                                    |
| ---------- | ------------------------------------------------------- |
| Players    | 100–300 concurrent across multiple cells                |
| Realms     | Genesis Voxel Realm + one experimental portal realm     |
| Governance | Changeable voting rules with activation delay           |
| AI         | Prompt-to-item, prompt-to-biome, prompt-to-NPC behavior |
| Patches    | Live assets, data, and Wasm gameplay modules            |
| Economy    | Internal Reality Tokens and reputation                  |
| Storage    | Content-addressed asset packs pinned by peers           |
| Moderation | Proposal review, warnings, blocklists, local forks      |
| Forking    | Players can follow alternate accepted ledgers           |

## What to build first

Build in this exact order:

Rust/Bevy voxel client
Player identity and signed actions
P2P lobby and world cell sync
Proposal manifest format
Voting UI
Accepted proposal ledger
Content-addressed patch packs
Wasm plugin ABI
Asset hot patch
Gameplay Wasm hot patch
Portal to second realm
Governance-rule patching
Reputation economy