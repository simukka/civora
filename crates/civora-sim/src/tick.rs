use crate::action::Action;
use crate::chunk::{CHUNK_SIZE, ChunkPos};
use crate::world::VoxelWorld;

/// Apply one simulation step: actions are applied in order, deterministically.
///
/// Returns the chunks whose contents (or visible borders) changed, sorted and
/// deduplicated. When an edit touches a chunk border the neighboring chunk is
/// included, since its boundary faces may need re-evaluation by the renderer.
pub fn step(world: &mut VoxelWorld, actions: &[Action]) -> Vec<ChunkPos> {
    let mut dirty: Vec<ChunkPos> = Vec::new();

    for action in actions {
        let changed = match *action {
            Action::PlaceBlock { pos, block } => {
                if block.is_solid() && world.get_block(pos).is_air() {
                    world.set_block(pos, block).map(|c| (pos, c))
                } else {
                    None
                }
            }
            Action::BreakBlock { pos } => {
                if world.get_block(pos).is_solid() {
                    world
                        .set_block(pos, crate::block::BlockId::AIR)
                        .map(|c| (pos, c))
                } else {
                    None
                }
            }
        };

        if let Some((pos, chunk_pos)) = changed {
            dirty.push(chunk_pos);
            for neighbor in border_neighbors(pos, chunk_pos) {
                dirty.push(neighbor);
            }
        }
    }

    dirty.sort();
    dirty.dedup();
    dirty
}

/// Chunks adjacent to `chunk_pos` that share a face with the block at `pos`.
fn border_neighbors(pos: [i32; 3], chunk_pos: ChunkPos) -> impl Iterator<Item = ChunkPos> {
    let local = [
        pos[0].rem_euclid(CHUNK_SIZE),
        pos[1].rem_euclid(CHUNK_SIZE),
        pos[2].rem_euclid(CHUNK_SIZE),
    ];
    (0..3).filter_map(move |axis| {
        let offset = if local[axis] == 0 {
            -1
        } else if local[axis] == CHUNK_SIZE - 1 {
            1
        } else {
            return None;
        };
        let mut neighbor = chunk_pos;
        match axis {
            0 => neighbor.x += offset,
            1 => neighbor.y += offset,
            _ => neighbor.z += offset,
        }
        Some(neighbor)
    })
}
