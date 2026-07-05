use civora_sim::chunk::world_to_local;
use civora_sim::{Action, BlockId, CHUNK_SIZE, ChunkPos, VoxelWorld, raycast, tick};

#[test]
fn coord_math_handles_negative_positions() {
    let (chunk, local) = world_to_local([-1, 0, CHUNK_SIZE]);
    assert_eq!(chunk, ChunkPos::new(-1, 0, 1));
    assert_eq!(local, [CHUNK_SIZE as usize - 1, 0, 0]);

    let (chunk, local) = world_to_local([-CHUNK_SIZE, -CHUNK_SIZE - 1, 0]);
    assert_eq!(chunk, ChunkPos::new(-1, -2, 0));
    assert_eq!(local, [0, CHUNK_SIZE as usize - 1, 0]);
}

#[test]
fn set_get_across_chunk_borders() {
    let mut world = VoxelWorld::new();
    for pos in [
        [0, 0, 0],
        [-1, 0, 0],
        [CHUNK_SIZE - 1, 0, 0],
        [CHUNK_SIZE, 0, 0],
        [5, -1, 5],
        [5, CHUNK_SIZE, 5],
    ] {
        world.set_block(pos, BlockId::STONE);
        assert_eq!(world.get_block(pos), BlockId::STONE, "at {pos:?}");
    }
    assert_eq!(world.get_block([1, 0, 0]), BlockId::AIR);
}

#[test]
fn flat_world_layers() {
    let world = VoxelWorld::flat(1);
    assert_eq!(world.get_block([0, -1, 0]), BlockId::STONE);
    assert_eq!(world.get_block([0, 0, 0]), BlockId::DIRT);
    assert_eq!(world.get_block([0, 2, 0]), BlockId::DIRT);
    assert_eq!(world.get_block([0, 3, 0]), BlockId::GRASS);
    assert_eq!(world.get_block([0, 4, 0]), BlockId::AIR);
    // Outside the generated radius reads as air.
    assert_eq!(world.get_block([CHUNK_SIZE * 2, 3, 0]), BlockId::AIR);
}

#[test]
fn content_hash_is_insertion_order_independent() {
    let mut a = VoxelWorld::new();
    a.set_block([0, 0, 0], BlockId::STONE);
    a.set_block([100, 5, -40], BlockId::DIRT);

    let mut b = VoxelWorld::new();
    b.set_block([100, 5, -40], BlockId::DIRT);
    b.set_block([0, 0, 0], BlockId::STONE);

    assert_eq!(a.content_hash(), b.content_hash());

    b.set_block([0, 0, 0], BlockId::GRASS);
    assert_ne!(a.content_hash(), b.content_hash());
}

#[test]
fn replaying_actions_is_deterministic() {
    let actions = vec![
        Action::PlaceBlock {
            pos: [1, 4, 1],
            block: BlockId::PLANK,
        },
        Action::BreakBlock { pos: [1, 3, 1] },
        Action::PlaceBlock {
            pos: [1, 3, 1],
            block: BlockId::GLASS,
        },
        Action::PlaceBlock {
            pos: [-5, 4, 7],
            block: BlockId::STONE,
        },
        Action::BreakBlock { pos: [0, -1, 0] },
    ];

    let mut a = VoxelWorld::flat(2);
    let mut b = VoxelWorld::flat(2);
    tick::step(&mut a, &actions);
    tick::step(&mut b, &actions);
    assert_eq!(a.content_hash(), b.content_hash());
}

#[test]
fn step_applies_action_semantics() {
    let mut world = VoxelWorld::flat(1);

    // Place into air succeeds.
    let dirty = tick::step(
        &mut world,
        &[Action::PlaceBlock {
            pos: [1, 4, 1],
            block: BlockId::PLANK,
        }],
    );
    assert_eq!(world.get_block([1, 4, 1]), BlockId::PLANK);
    assert!(dirty.contains(&ChunkPos::new(0, 0, 0)));

    // Place into an occupied cell is rejected.
    let dirty = tick::step(
        &mut world,
        &[Action::PlaceBlock {
            pos: [1, 4, 1],
            block: BlockId::STONE,
        }],
    );
    assert_eq!(world.get_block([1, 4, 1]), BlockId::PLANK);
    assert!(dirty.is_empty());

    // Break solid succeeds; break air is a no-op.
    let dirty = tick::step(&mut world, &[Action::BreakBlock { pos: [1, 4, 1] }]);
    assert_eq!(world.get_block([1, 4, 1]), BlockId::AIR);
    assert!(!dirty.is_empty());
    let dirty = tick::step(&mut world, &[Action::BreakBlock { pos: [1, 4, 1] }]);
    assert!(dirty.is_empty());
}

#[test]
fn border_edit_dirties_neighbor_chunk() {
    let mut world = VoxelWorld::flat(1);
    // y = 0 is the bottom face of chunk (0,0,0); breaking it exposes chunk (0,-1,0).
    let dirty = tick::step(&mut world, &[Action::BreakBlock { pos: [0, 0, 0] }]);
    assert!(dirty.contains(&ChunkPos::new(0, 0, 0)));
    assert!(dirty.contains(&ChunkPos::new(0, -1, 0)));
    assert!(dirty.contains(&ChunkPos::new(-1, 0, 0)));
    assert!(dirty.contains(&ChunkPos::new(0, 0, -1)));
}

#[test]
fn action_encoding_round_trips() {
    let actions = [
        Action::PlaceBlock {
            pos: [-5, 3, 1_000_000],
            block: BlockId::GLASS,
        },
        Action::PlaceBlock {
            pos: [0, 0, 0],
            block: BlockId::AIR,
        },
        Action::BreakBlock {
            pos: [i32::MIN, -1, i32::MAX],
        },
    ];
    for action in actions {
        let mut bytes = Vec::new();
        action.encode(&mut bytes);
        assert_eq!(Action::decode(&bytes), Some(action), "for {action:?}");
    }
}

#[test]
fn action_decode_rejects_malformed_input() {
    let mut bytes = Vec::new();
    Action::PlaceBlock {
        pos: [1, 2, 3],
        block: BlockId::STONE,
    }
    .encode(&mut bytes);

    assert_eq!(Action::decode(&[]), None);
    assert_eq!(Action::decode(&bytes[..bytes.len() - 1]), None); // truncated

    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(Action::decode(&trailing), None); // trailing bytes

    let mut bad_tag = bytes.clone();
    bad_tag[0] = 9;
    assert_eq!(Action::decode(&bad_tag), None); // unknown tag
}

#[test]
fn raycast_hits_ground_from_above() {
    let world = VoxelWorld::flat(1);
    let hit =
        raycast(&world, [0.5, 10.0, 0.5], [0.0, -1.0, 0.0], 20.0).expect("should hit the ground");
    assert_eq!(hit.pos, [0, 3, 0]);
    assert_eq!(hit.normal, [0, 1, 0]);
    assert!((hit.distance - 6.0).abs() < 1e-4);
}

#[test]
fn raycast_hits_side_face() {
    let mut world = VoxelWorld::flat(1);
    world.set_block([5, 5, 0], BlockId::STONE);
    let hit = raycast(&world, [0.5, 5.5, 0.5], [1.0, 0.0, 0.0], 20.0)
        .expect("should hit the placed block");
    assert_eq!(hit.pos, [5, 5, 0]);
    assert_eq!(hit.normal, [-1, 0, 0]);
}

#[test]
fn raycast_respects_max_distance_and_misses_sky() {
    let world = VoxelWorld::flat(1);
    assert!(raycast(&world, [0.5, 10.0, 0.5], [0.0, -1.0, 0.0], 3.0).is_none());
    assert!(raycast(&world, [0.5, 10.0, 0.5], [0.0, 1.0, 0.0], 100.0).is_none());
}

#[test]
fn raycast_from_inside_solid_reports_zero_normal() {
    let world = VoxelWorld::flat(1);
    let hit = raycast(&world, [0.5, 0.5, 0.5], [0.0, 1.0, 0.0], 10.0).expect("inside solid");
    assert_eq!(hit.pos, [0, 0, 0]);
    assert_eq!(hit.normal, [0, 0, 0]);
    assert_eq!(hit.distance, 0.0);
}

/// `CHUNK_SIZE` is a protocol-level constant, not a local tuning knob.
///
/// It feeds `VoxelWorld::content_hash` (via chunk coordinates), so two peers
/// running different values would produce different world hashes for identical
/// block content, breaking content-addressing, snapshots, and finality
/// certificates. Changing it is a breaking protocol change and must be a
/// deliberate, coordinated decision — this test fails loudly if it drifts.
#[test]
fn chunk_size_is_pinned_to_32() {
    assert_eq!(
        CHUNK_SIZE, 32,
        "CHUNK_SIZE is a protocol constant; changing it breaks cross-peer \
         content hashes and world snapshots. Update the protocol version and \
         all peers deliberately, not just this test."
    );
}
