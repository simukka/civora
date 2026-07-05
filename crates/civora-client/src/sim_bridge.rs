//! Bridge between the Bevy app and the deterministic simulation core.
//!
//! The client never mutates [`civora_sim::VoxelWorld`] directly: input
//! systems push [`Action`]s onto [`ActionQueue`], and the queue is drained
//! through [`civora_sim::tick::step`] on `FixedUpdate`. This is the seam
//! where signed actions and cell-committee validation slot in later.

use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use civora_sim::{Action, ChunkPos, VoxelWorld, tick};

/// How many chunks of flat test world to generate around the origin.
const WORLD_RADIUS_CHUNKS: i32 = 3;

pub struct SimBridgePlugin;

impl Plugin for SimBridgePlugin {
    fn build(&self, app: &mut App) {
        let world = VoxelWorld::flat(WORLD_RADIUS_CHUNKS);
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

/// Player intents waiting for the next simulation tick.
#[derive(Resource, Default)]
pub struct ActionQueue(pub Vec<Action>);

/// Chunks whose meshes are stale. The renderer drains this set.
#[derive(Resource, Default)]
pub struct DirtyChunks(pub HashSet<ChunkPos>);

fn drain_action_queue(
    mut world: ResMut<SimWorld>,
    mut queue: ResMut<ActionQueue>,
    mut dirty: ResMut<DirtyChunks>,
) {
    if queue.0.is_empty() {
        return;
    }
    let actions = std::mem::take(&mut queue.0);
    for chunk_pos in tick::step(&mut world.0, &actions) {
        dirty.0.insert(chunk_pos);
    }
}
