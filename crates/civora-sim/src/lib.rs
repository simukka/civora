//! Deterministic voxel simulation core for Civora.
//!
//! This crate is the seed of the Reality Kernel's deterministic scheduler:
//! no Bevy, no I/O, no external dependencies. All world mutation goes
//! through [`tick::step`] so the action stream can later be signed,
//! gossiped, and validated by cell committees.

pub mod action;
pub mod block;
pub mod chunk;
pub mod raycast;
pub mod tick;
pub mod world;

pub use action::Action;
pub use block::BlockId;
pub use chunk::{CHUNK_SIZE, Chunk, ChunkPos};
pub use raycast::{Hit, raycast};
pub use world::VoxelWorld;
