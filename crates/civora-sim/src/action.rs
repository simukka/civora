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

impl Action {
    /// Append the canonical byte encoding of this action to `out`.
    ///
    /// This is the exact byte string that identity keys sign, so it must be
    /// canonical: every action has exactly one encoding (tag byte, then
    /// little-endian `i32` coordinates, then the block id for placements).
    pub fn encode(&self, out: &mut Vec<u8>) {
        match *self {
            Action::PlaceBlock { pos, block } => {
                out.push(0);
                for coord in pos {
                    out.extend_from_slice(&coord.to_le_bytes());
                }
                out.push(block.0);
            }
            Action::BreakBlock { pos } => {
                out.push(1);
                for coord in pos {
                    out.extend_from_slice(&coord.to_le_bytes());
                }
            }
        }
    }

    /// Decode a canonical encoding produced by [`Action::encode`].
    ///
    /// Returns `None` for unknown tags, truncated input, or trailing bytes,
    /// so decode(encode(a)) round-trips and nothing else parses.
    pub fn decode(bytes: &[u8]) -> Option<Action> {
        fn pos(bytes: &[u8]) -> [i32; 3] {
            let coord = |i: usize| {
                let mut le = [0u8; 4];
                le.copy_from_slice(&bytes[i * 4..i * 4 + 4]);
                i32::from_le_bytes(le)
            };
            [coord(0), coord(1), coord(2)]
        }

        let (&tag, rest) = bytes.split_first()?;
        match tag {
            0 if rest.len() == 13 => Some(Action::PlaceBlock {
                pos: pos(&rest[..12]),
                block: BlockId(rest[12]),
            }),
            1 if rest.len() == 12 => Some(Action::BreakBlock { pos: pos(rest) }),
            _ => None,
        }
    }
}
