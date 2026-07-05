use crate::block::BlockId;

/// A player intent that mutates the world.
///
/// All world mutation flows through actions (see [`crate::tick::step`]).
/// In later milestones actions are signed by the player's identity key and
/// gossiped to the cell committee; keeping the type plain data now makes
/// that a wrapper, not a rewrite.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Place `block` at `pos`. Applies only if the target is currently air.
    PlaceBlock { pos: [i32; 3], block: BlockId },
    /// Break the block at `pos`. Applies only if the target is solid.
    BreakBlock { pos: [i32; 3] },
}
