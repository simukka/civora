use std::collections::HashMap;

use crate::block::BlockId;
use crate::chunk::{CHUNK_SIZE, Chunk, ChunkPos, world_to_local};

/// The voxel world: a sparse map of dense chunks.
///
/// Positions outside any allocated chunk read as air. Setting a block in a
/// missing chunk allocates it.
#[derive(Default)]
pub struct VoxelWorld {
    chunks: HashMap<ChunkPos, Chunk>,
}

impl VoxelWorld {
    pub fn new() -> Self {
        Self::default()
    }

    /// Flat test world: `radius` chunks in every horizontal direction from
    /// the origin. Stone fills world y < 0, dirt fills y 0..3, grass caps
    /// y = 3, air above.
    pub fn flat(radius: i32) -> Self {
        let mut world = Self::new();
        for cx in -radius..=radius {
            for cz in -radius..=radius {
                world
                    .chunks
                    .insert(ChunkPos::new(cx, -1, cz), Chunk::filled(BlockId::STONE));

                let mut surface = Chunk::empty();
                for x in 0..CHUNK_SIZE as usize {
                    for z in 0..CHUNK_SIZE as usize {
                        surface.set([x, 0, z], BlockId::DIRT);
                        surface.set([x, 1, z], BlockId::DIRT);
                        surface.set([x, 2, z], BlockId::DIRT);
                        surface.set([x, 3, z], BlockId::GRASS);
                    }
                }
                world.chunks.insert(ChunkPos::new(cx, 0, cz), surface);
            }
        }
        world
    }

    pub fn get_block(&self, pos: [i32; 3]) -> BlockId {
        let (chunk_pos, local) = world_to_local(pos);
        match self.chunks.get(&chunk_pos) {
            Some(chunk) => chunk.get(local),
            None => BlockId::AIR,
        }
    }

    /// Set a block, allocating the chunk if needed. Returns the chunk that
    /// changed, or `None` if the block already had that value.
    pub fn set_block(&mut self, pos: [i32; 3], block: BlockId) -> Option<ChunkPos> {
        let (chunk_pos, local) = world_to_local(pos);
        let chunk = self.chunks.entry(chunk_pos).or_insert_with(Chunk::empty);
        if chunk.get(local) == block {
            return None;
        }
        chunk.set(local, block);
        Some(chunk_pos)
    }

    /// Install a whole chunk, replacing any existing one at `pos`.
    ///
    /// Used when applying a world snapshot received from a peer; live edits
    /// still go through [`VoxelWorld::set_block`] via actions.
    pub fn insert_chunk(&mut self, pos: ChunkPos, chunk: Chunk) {
        self.chunks.insert(pos, chunk);
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    pub fn chunk_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks.keys().copied()
    }

    /// Deterministic content hash (FNV-1a) over chunks in sorted order.
    /// Two worlds with identical block contents hash identically regardless
    /// of insertion order.
    pub fn content_hash(&self) -> u64 {
        const FNV_OFFSET: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x100000001b3;

        let mut hash = FNV_OFFSET;
        let mut byte = |b: u8| {
            hash ^= b as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        };

        let mut positions: Vec<ChunkPos> = self.chunks.keys().copied().collect();
        positions.sort();
        for pos in positions {
            let chunk = &self.chunks[&pos];
            // Skip fully-empty chunks so "allocated but air" == "missing".
            if chunk.is_empty() {
                continue;
            }
            for coord in [pos.x, pos.y, pos.z] {
                for b in coord.to_le_bytes() {
                    byte(b);
                }
            }
            for b in chunk.block_bytes() {
                byte(b);
            }
        }
        hash
    }
}
