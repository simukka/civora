/// A block type identifier. `0` is always air.
///
/// For milestone 1 this is a fixed set of built-in blocks. Later, the
/// registry becomes data-driven so voted-in proposals can add block types.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct BlockId(pub u8);

impl BlockId {
    pub const AIR: BlockId = BlockId(0);
    pub const GRASS: BlockId = BlockId(1);
    pub const DIRT: BlockId = BlockId(2);
    pub const STONE: BlockId = BlockId(3);
    pub const PLANK: BlockId = BlockId(4);
    pub const GLASS: BlockId = BlockId(5);

    /// Blocks a player can select in the hotbar.
    pub const PLACEABLE: [BlockId; 5] = [
        Self::GRASS,
        Self::DIRT,
        Self::STONE,
        Self::PLANK,
        Self::GLASS,
    ];

    pub fn is_air(self) -> bool {
        self == Self::AIR
    }

    pub fn is_solid(self) -> bool {
        !self.is_air()
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::AIR => "air",
            Self::GRASS => "grass",
            Self::DIRT => "dirt",
            Self::STONE => "stone",
            Self::PLANK => "plank",
            Self::GLASS => "glass",
            _ => "unknown",
        }
    }
}
