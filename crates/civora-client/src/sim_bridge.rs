//! Bridge between the Bevy app and the deterministic simulation core.
//!
//! The client never mutates [`civora_sim::VoxelWorld`] directly: input
//! systems push [`Action`]s onto [`ActionQueue`]; on `FixedUpdate` each
//! action is signed with the player's identity key, verified into the
//! session's [`SessionLog`] (the Reality Kernel gate — an action that fails
//! verification never reaches the world), and only then applied through
//! [`civora_sim::tick::step`]. Cell-committee validation slots in here later.

use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use civora_net::NetCommand;
use civora_sim::{Action, ChunkPos, VoxelWorld, tick};

use crate::identity::{LocalIdentity, SessionLog};
use crate::net::{NetChannels, NetStatus};

/// How many chunks of flat test world to generate around the origin.
const WORLD_RADIUS_CHUNKS: i32 = 3;

pub struct SimBridgePlugin {
    /// Joiners start empty and receive the real world via sync; hosts and
    /// single players generate the flat test world.
    pub start_empty: bool,
}

impl Plugin for SimBridgePlugin {
    fn build(&self, app: &mut App) {
        let world = if self.start_empty {
            VoxelWorld::new()
        } else {
            VoxelWorld::flat(WORLD_RADIUS_CHUNKS)
        };
        // Everything starts dirty so the renderer meshes the initial world.
        let dirty: HashSet<ChunkPos> = world.chunk_positions().collect();

        app.insert_resource(SimWorld(world))
            .insert_resource(ActionQueue::default())
            .insert_resource(DirtyChunks(dirty))
            .add_systems(FixedUpdate, drain_action_queue);
    }
}

/// The authoritative world state, owned by the sim core.
#[derive(Resource)]
pub struct SimWorld(pub VoxelWorld);

/// Generate the flat test world in place (the menu's host/offline path;
/// plugin-time generation covers the CLI paths).
pub fn generate_flat_world(world: &mut SimWorld, dirty: &mut DirtyChunks) {
    world.0 = VoxelWorld::flat(WORLD_RADIUS_CHUNKS);
    dirty.0.extend(world.0.chunk_positions());
}

/// Player intents waiting for the next simulation tick.
#[derive(Resource, Default)]
pub struct ActionQueue(pub Vec<Action>);

/// Chunks whose meshes are stale. The renderer drains this set.
#[derive(Resource, Default)]
pub struct DirtyChunks(pub HashSet<ChunkPos>);

pub(crate) fn drain_action_queue(
    mut world: ResMut<SimWorld>,
    mut queue: ResMut<ActionQueue>,
    mut dirty: ResMut<DirtyChunks>,
    mut local: ResMut<LocalIdentity>,
    mut log: ResMut<SessionLog>,
    status: Res<NetStatus>,
    channels: Option<Res<NetChannels>>,
) {
    if queue.0.is_empty() {
        return;
    }
    // While joining there is no world to act on yet; the queue is also
    // emptied so pre-sync clicks don't fire into the synced world.
    if status.gate_input() {
        queue.0.clear();
        return;
    }
    let mut verified = Vec::with_capacity(queue.0.len());
    for action in std::mem::take(&mut queue.0) {
        let signed = local.identity.sign(action, local.next_seq);
        // Kernel gate: only actions accepted into the signed log (valid
        // signature, strictly increasing seq) may mutate the world.
        match log.0.append(signed) {
            Ok(()) => {
                local.next_seq += 1;
                verified.push(action);
                // Gossip what passed the gate; peers re-verify on their end.
                if let Some(channels) = &channels {
                    let _ = channels.commands.send(NetCommand::PublishAction(signed));
                }
            }
            Err(err) => warn!("rejected action: {err}"),
        }
    }
    for chunk_pos in tick::step(&mut world.0, &verified) {
        dirty.0.insert(chunk_pos);
    }
}
