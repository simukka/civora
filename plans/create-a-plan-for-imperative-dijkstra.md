# Plan: Buildable, Movable Voxel Objects

An *object* is any dynamic voxel body distinct from the single static world that exists today — a ship, a floating island, a car, a mech, a planet. They all share one abstraction: an object is its own integer voxel grid ("volume") carrying a pose. Players can **spawn on** it (ride/stand), **control** it (drive/pilot/spin), and **change** it (build/break) — the same interactions the static world already supports, now relative to a moving frame. Generalize [civora-sim](crates/civora-sim/src/world.rs) from one `VoxelWorld` to many volumes, make edits/raycast/collision frame-aware, fold every volume into `content_hash`, and parent each object's chunk meshes plus any riding players under a Bevy transform. Because the whole stack currently assumes voxel-space == world-space, the pivotal choice is whether an object's pose is authoritative-deterministic or client-local like the player position is today.

## Steps

1. Add a `Volume { id, VoxelWorld, pose }` type in [world.rs](crates/civora-sim/src/world.rs); keep volume 0 as the static world and treat every other volume as a dynamic object (ship, island, car, mech, planet). Thread a `VolumeId` through get/set/`raycast`.
2. Extend `Action` in [action.rs](crates/civora-sim/src/action.rs) with a volume selector plus object lifecycle/control variants (spawn volume, control input → pose, attach/detach a player); version `encode`/`decode` and the wire proto in [wire.rs](crates/civora-net/src/wire.rs).
3. Fold each volume's grid and fixed-point pose into [content_hash](crates/civora-sim/src/world.rs) in sorted-`VolumeId` order; integrate pose on the 20 Hz `FixedUpdate` tick.
4. Make [raycast](crates/civora-sim/src/raycast.rs#L20) and the [collision sweep](crates/civora-client/src/player.rs) frame-aware — transform ray/AABB into the target volume's local space, solve, map hit/push-out back, pick nearest across all volumes. This is what lets a player build on and walk around a moving object.
5. Parent each volume's chunk meshes — and any players spawned on or riding it — under that volume's root `Transform` in [render.rs](crates/civora-client/src/render.rs) and [player.rs](crates/civora-client/src/player.rs), so riders move with the object and a controlling player drives its pose.
6. Serialize every volume (grids + poses + player attachments) in canonical order in net snapshots, gated through `ActionLog::append` like every other edit.

## Player interactions

Every object supports the same three roles; a given object may allow one, two, or all three:

1. **Spawn on / ride** — the player's frame parents to the volume, so they stand on a floating island or inside a ship and move with it (kinematic carry, not inertial physics in v1).
2. **Control** — player input drives the volume's pose: drive a car, pilot a ship, walk a mech, spin a planet. Only translation + orientation change; the voxels stay put.
3. **Change** — build/break voxels in the volume's local frame, identical to editing the static world but routed through the frame-aware raycast + action path.

## Further Considerations

1. Object pose authority? Option A: authoritative-deterministic (fixed-point, folded into `content_hash`, log-driven — fully synced, no divergence). Option B: client-local like player position today (simpler, but objects aren't shared authoritative state). This drives how much of steps 2–3 and 6 are needed.
2. Rotation model? Option A: 90°-snapped/axis-aligned orientation keeps integer collision cheap (fine for cars, platforms, most builds). Option B: free 6-DoF quaternion needs rotated-voxel collision and full transform math — required for a tumbling ship or a spinning planet, and no physics engine exists yet (rules out f32 rapier/avian if the pose is hashed).
3. Rigid vs. articulated objects? A car, island, or planet is one rigid volume; a mech is several child volumes (torso, limbs) with jointed poses. The `Volume` + pose hierarchy composes for this, but joints/articulation are a later layer — scope v1 to single rigid volumes.
4. Scale extremes? A planet is just a very large (or voxel-sphere) volume that mostly rotates; confirm chunk streaming and the 32³ `CHUNK_SIZE` protocol constant hold up before treating planet-scale as in-scope.
5. Protocol/governance? Changing `Action`, snapshots, and `content_hash` is a wire break — bump the proto/`CellRef` version, and possibly ship it as a signed proposal per the governance model rather than a silent format change.
