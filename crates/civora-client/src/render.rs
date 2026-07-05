//! Chunk meshing and world rendering.
//!
//! Milestone-1 mesher: culled faces with per-vertex colors. One mesh entity
//! per chunk. Greedy meshing and texture atlases come later.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use civora_sim::{BlockId, CHUNK_SIZE, ChunkPos, VoxelWorld};

use crate::sim_bridge::{DirtyChunks, SimWorld};

pub struct VoxelRenderPlugin;

impl Plugin for VoxelRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChunkMeshIndex>()
            .add_systems(Startup, setup_lighting_and_material)
            .add_systems(Update, remesh_dirty_chunks);
    }
}

/// Mesh entity per chunk, so remeshing can replace or despawn it.
#[derive(Resource, Default)]
pub struct ChunkMeshIndex(HashMap<ChunkPos, Entity>);

/// One shared material for all chunks; color comes from vertex attributes.
#[derive(Resource)]
struct ChunkMaterial(Handle<StandardMaterial>);

fn setup_lighting_and_material(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        DirectionalLight {
            illuminance: 8_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.3, -0.8, -0.9)),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 700.0,
        ..default()
    });
    commands.insert_resource(ChunkMaterial(materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.95,
        reflectance: 0.05,
        ..default()
    })));
}

fn remesh_dirty_chunks(
    mut commands: Commands,
    world: Res<SimWorld>,
    mut dirty: ResMut<DirtyChunks>,
    mut index: ResMut<ChunkMeshIndex>,
    mut meshes: ResMut<Assets<Mesh>>,
    material: Res<ChunkMaterial>,
) {
    if dirty.0.is_empty() {
        return;
    }
    for chunk_pos in dirty.0.drain() {
        let existing = index.0.get(&chunk_pos).copied();
        match build_chunk_mesh(&world.0, chunk_pos) {
            Some(mesh) => {
                let handle = meshes.add(mesh);
                match existing {
                    Some(entity) => {
                        commands.entity(entity).insert(Mesh3d(handle));
                    }
                    None => {
                        let min = chunk_pos.world_min();
                        let entity = commands
                            .spawn((
                                Mesh3d(handle),
                                MeshMaterial3d(material.0.clone()),
                                Transform::from_xyz(min[0] as f32, min[1] as f32, min[2] as f32),
                            ))
                            .id();
                        index.0.insert(chunk_pos, entity);
                    }
                }
            }
            None => {
                if let Some(entity) = existing {
                    commands.entity(entity).despawn();
                    index.0.remove(&chunk_pos);
                }
            }
        }
    }
}

/// Per-face vertex offsets from the block's min corner, counter-clockwise
/// seen from outside, plus the outward normal. Order: +X -X +Y -Y +Z -Z.
const FACES: [([i32; 3], [[f32; 3]; 4]); 6] = [
    (
        [1, 0, 0],
        [
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
        ],
    ),
    (
        [-1, 0, 0],
        [
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0],
        ],
    ),
    (
        [0, 1, 0],
        [
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
    ),
    (
        [0, -1, 0],
        [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ],
    ),
    (
        [0, 0, 1],
        [
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ],
    ),
    (
        [0, 0, -1],
        [
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ],
    ),
];

/// Simple directional shading baked into vertex colors, per face.
fn face_shade(normal: [i32; 3]) -> f32 {
    match normal {
        [0, 1, 0] => 1.0,
        [0, -1, 0] => 0.45,
        [_, 0, 0] => 0.8,
        _ => 0.62,
    }
}

pub fn block_color(block: BlockId) -> [f32; 3] {
    match block {
        BlockId::GRASS => [0.32, 0.62, 0.24],
        BlockId::DIRT => [0.46, 0.32, 0.21],
        BlockId::STONE => [0.55, 0.55, 0.58],
        BlockId::PLANK => [0.72, 0.55, 0.34],
        BlockId::GLASS => [0.68, 0.85, 0.94],
        _ => [1.0, 0.0, 1.0],
    }
}

/// Build the culled-face mesh for one chunk, in chunk-local coordinates.
/// Returns `None` when the chunk has no visible faces.
fn build_chunk_mesh(world: &VoxelWorld, chunk_pos: ChunkPos) -> Option<Mesh> {
    let chunk = world.chunk(chunk_pos)?;
    if chunk.is_empty() {
        return None;
    }

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let min = chunk_pos.world_min();
    for y in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let block = chunk.get([x as usize, y as usize, z as usize]);
                if block.is_air() {
                    continue;
                }
                let color = block_color(block);
                for (normal, corners) in FACES {
                    // Neighbor lookup goes through the world so faces on
                    // chunk borders are culled against adjacent chunks.
                    let neighbor = [
                        min[0] + x + normal[0],
                        min[1] + y + normal[1],
                        min[2] + z + normal[2],
                    ];
                    if world.get_block(neighbor).is_solid() {
                        continue;
                    }
                    let shade = face_shade(normal);
                    let base = positions.len() as u32;
                    for corner in corners {
                        positions.push([
                            x as f32 + corner[0],
                            y as f32 + corner[1],
                            z as f32 + corner[2],
                        ]);
                        normals.push([normal[0] as f32, normal[1] as f32, normal[2] as f32]);
                        colors.push([color[0] * shade, color[1] * shade, color[2] * shade, 1.0]);
                    }
                    indices.extend_from_slice(&[
                        base,
                        base + 1,
                        base + 2,
                        base,
                        base + 2,
                        base + 3,
                    ]);
                }
            }
        }
    }

    if positions.is_empty() {
        return None;
    }

    Some(
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(indices)),
    )
}
