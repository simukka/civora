use std::collections::HashMap;

use civora_sim::{ChunkPos, VoxelWorld, tick};

use crate::identity::PlayerId;
use crate::signed::{SignedAction, VerifyError};

/// Append-only log of verified signed actions.
///
/// Nothing enters the log without a valid signature and a strictly
/// increasing per-author sequence number. In the P2P milestone this log is
/// what gets gossiped to and validated by cell committees ("voxel edits =
/// signed action log + periodic snapshots").
#[derive(Default)]
pub struct ActionLog {
    entries: Vec<SignedAction>,
    last_seq: HashMap<PlayerId, u64>,
}

impl ActionLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Verify `entry` and append it.
    ///
    /// Rejects invalid signatures and sequence numbers that are not strictly
    /// greater than the author's last accepted one (anti-replay).
    pub fn append(&mut self, entry: SignedAction) -> Result<(), VerifyError> {
        entry.verify()?;
        if let Some(&last) = self.last_seq.get(&entry.author)
            && entry.seq <= last
        {
            return Err(VerifyError::SeqReplay {
                author: entry.author,
                seq: entry.seq,
            });
        }
        self.last_seq.insert(entry.author, entry.seq);
        self.entries.push(entry);
        Ok(())
    }

    pub fn entries(&self) -> &[SignedAction] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Re-verify every entry (signatures and sequence order) and replay the
    /// actions onto `world` in log order.
    ///
    /// A verified log replayed onto the same starting world reproduces the
    /// same [`VoxelWorld::content_hash`] — the determinism proof that later
    /// lets any peer audit a cell's history. Returns the dirtied chunks,
    /// sorted and deduplicated.
    pub fn verify_and_replay(&self, world: &mut VoxelWorld) -> Result<Vec<ChunkPos>, VerifyError> {
        let mut last_seq: HashMap<PlayerId, u64> = HashMap::new();
        let mut dirty = Vec::new();
        for entry in &self.entries {
            entry.verify()?;
            if let Some(&last) = last_seq.get(&entry.author)
                && entry.seq <= last
            {
                return Err(VerifyError::SeqReplay {
                    author: entry.author,
                    seq: entry.seq,
                });
            }
            last_seq.insert(entry.author, entry.seq);
            dirty.extend(tick::step(world, &[entry.action]));
        }
        dirty.sort();
        dirty.dedup();
        Ok(dirty)
    }
}
