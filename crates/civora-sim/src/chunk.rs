use crate::block::BlockId;

pub const CHUNK_SIZE: i32 = 32;
const SIZE: usize = CHUNK_SIZE as usize;
const VOLUME: usize = SIZE * SIZE * SIZE;

/// Position of a chunk in chunk coordinates (world position / CHUNK_SIZE).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ChunkPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkPos {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn from_world(pos: [i32; 3]) -> Self {
        Self {
            x: pos[0].div_euclid(CHUNK_SIZE),
            y: pos[1].div_euclid(CHUNK_SIZE),
            z: pos[2].div_euclid(CHUNK_SIZE),
        }
    }

    /// World position of this chunk's minimum corner.
    pub fn world_min(self) -> [i32; 3] {
        [
            self.x * CHUNK_SIZE,
            self.y * CHUNK_SIZE,
            self.z * CHUNK_SIZE,
        ]
    }
}

/// Split a world position into its chunk and the local offset inside it.
pub fn world_to_local(pos: [i32; 3]) -> (ChunkPos, [usize; 3]) {
    let chunk = ChunkPos::from_world(pos);
    let local = [
        pos[0].rem_euclid(CHUNK_SIZE) as usize,
        pos[1].rem_euclid(CHUNK_SIZE) as usize,
        pos[2].rem_euclid(CHUNK_SIZE) as usize,
    ];
    (chunk, local)
}

/// A dense 32x32x32 block array.
#[derive(Clone)]
pub struct Chunk {
    blocks: Box<[BlockId; VOLUME]>,
    solid_count: u32,
}

impl Chunk {
    pub fn filled(block: BlockId) -> Self {
        Self {
            blocks: Box::new([block; VOLUME]),
            solid_count: if block.is_solid() { VOLUME as u32 } else { 0 },
        }
    }

    pub fn empty() -> Self {
        Self::filled(BlockId::AIR)
    }

    fn index(x: usize, y: usize, z: usize) -> usize {
        debug_assert!(x < SIZE && y < SIZE && z < SIZE);
        (y * SIZE + z) * SIZE + x
    }

    pub fn get(&self, local: [usize; 3]) -> BlockId {
        self.blocks[Self::index(local[0], local[1], local[2])]
    }

    pub fn set(&mut self, local: [usize; 3], block: BlockId) {
        let slot = &mut self.blocks[Self::index(local[0], local[1], local[2])];
        match (slot.is_solid(), block.is_solid()) {
            (false, true) => self.solid_count += 1,
            (true, false) => self.solid_count -= 1,
            _ => {}
        }
        *slot = block;
    }

    pub fn is_empty(&self) -> bool {
        self.solid_count == 0
    }

    /// Raw block bytes in deterministic index order (for hashing).
    pub fn block_bytes(&self) -> impl Iterator<Item = u8> + '_ {
        self.blocks.iter().map(|b| b.0)
    }
}
