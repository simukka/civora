//! First-person player: mouse look, walk/fly movement, voxel collision,
//! cursor grab. The player entity's translation is the center of its AABB;
//! the camera is a child at eye height.

use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions};
use civora_sim::VoxelWorld;

use crate::sim_bridge::SimWorld;

pub const PLAYER_HALF_EXTENTS: Vec3 = Vec3::new(0.3, 0.9, 0.3);
const EYE_ABOVE_CENTER: f32 = 0.72;
const WALK_SPEED: f32 = 5.6;
const FLY_SPEED: f32 = 12.0;
const GRAVITY: f32 = -24.0;
const JUMP_VELOCITY: f32 = 8.2;
const MOUSE_SENSITIVITY: f32 = 0.0025;
const SKIN: f32 = 1e-4;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_player)
            .add_systems(Update, (cursor_grab, mouse_look, movement).chain());
    }
}

#[derive(Component)]
pub struct Player {
    pub yaw: f32,
    pub pitch: f32,
    pub velocity: Vec3,
    pub flying: bool,
    pub grounded: bool,
}

/// Marker for the player's camera (used for raycasts and interaction).
#[derive(Component)]
pub struct PlayerCamera;

fn spawn_player(mut commands: Commands) {
    commands
        .spawn((
            Player {
                yaw: 0.0,
                pitch: 0.0,
                velocity: Vec3::ZERO,
                flying: false,
                grounded: false,
            },
            // Ground surface is y = 4; center the AABB just above it.
            Transform::from_xyz(0.5, 8.0, 0.5),
            Visibility::default(),
        ))
        .with_children(|parent| {
            parent.spawn((
                PlayerCamera,
                Camera3d::default(),
                Transform::from_xyz(0.0, EYE_ABOVE_CENTER, 0.0),
            ));
        });
}

pub(crate) fn cursor_grab(
    mut cursor_options: Single<&mut CursorOptions>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if mouse.just_pressed(MouseButton::Left) && cursor_options.grab_mode == CursorGrabMode::None {
        cursor_options.visible = false;
        cursor_options.grab_mode = CursorGrabMode::Locked;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor_options.visible = true;
        cursor_options.grab_mode = CursorGrabMode::None;
    }
}

pub fn cursor_grabbed(cursor_options: &CursorOptions) -> bool {
    cursor_options.grab_mode != CursorGrabMode::None
}

fn mouse_look(
    cursor_options: Single<&CursorOptions>,
    mut motions: MessageReader<MouseMotion>,
    mut player: Single<(&mut Player, &mut Transform), Without<PlayerCamera>>,
    mut camera: Single<&mut Transform, With<PlayerCamera>>,
) {
    if !cursor_grabbed(&cursor_options) {
        motions.clear();
        return;
    }
    let mut delta = Vec2::ZERO;
    for motion in motions.read() {
        delta += motion.delta;
    }
    if delta == Vec2::ZERO {
        return;
    }

    let (player, transform) = &mut *player;
    player.yaw -= delta.x * MOUSE_SENSITIVITY;
    player.pitch = (player.pitch - delta.y * MOUSE_SENSITIVITY).clamp(-1.54, 1.54);

    transform.rotation = Quat::from_rotation_y(player.yaw);
    camera.rotation = Quat::from_rotation_x(player.pitch);
}

fn movement(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    cursor_options: Single<&CursorOptions>,
    world: Res<SimWorld>,
    mut player: Single<(&mut Player, &mut Transform), Without<PlayerCamera>>,
) {
    let dt = time.delta_secs().min(0.05);
    let (player, transform) = &mut *player;
    let grabbed = cursor_grabbed(&cursor_options);

    if grabbed && keys.just_pressed(KeyCode::KeyF) {
        player.flying = !player.flying;
        player.velocity.y = 0.0;
    }

    // Horizontal wish direction in world space from WASD + yaw.
    let mut wish = Vec3::ZERO;
    if grabbed {
        if keys.pressed(KeyCode::KeyW) {
            wish.z -= 1.0;
        }
        if keys.pressed(KeyCode::KeyS) {
            wish.z += 1.0;
        }
        if keys.pressed(KeyCode::KeyA) {
            wish.x -= 1.0;
        }
        if keys.pressed(KeyCode::KeyD) {
            wish.x += 1.0;
        }
    }
    let wish = Quat::from_rotation_y(player.yaw) * wish;
    let wish = if wish.length_squared() > 0.0 {
        wish.normalize()
    } else {
        Vec3::ZERO
    };

    if player.flying {
        let mut velocity = wish * FLY_SPEED;
        if grabbed {
            if keys.pressed(KeyCode::Space) {
                velocity.y += FLY_SPEED;
            }
            if keys.pressed(KeyCode::ShiftLeft) {
                velocity.y -= FLY_SPEED;
            }
        }
        player.velocity = velocity;
    } else {
        player.velocity.x = wish.x * WALK_SPEED;
        player.velocity.z = wish.z * WALK_SPEED;
        player.velocity.y += GRAVITY * dt;
        if grabbed && player.grounded && keys.pressed(KeyCode::Space) {
            player.velocity.y = JUMP_VELOCITY;
        }
    }

    let delta = player.velocity * dt;
    let mut pos = transform.translation;
    player.grounded = false;

    for axis in 0..3 {
        if delta[axis] == 0.0 {
            continue;
        }
        let collided = move_axis(&world.0, &mut pos, axis, delta[axis]);
        if collided {
            if axis == 1 && delta.y < 0.0 {
                player.grounded = true;
            }
            player.velocity[axis] = 0.0;
        }
    }

    transform.translation = pos;
}

/// Move the AABB centered at `pos` along one axis, clamping against solid
/// voxels. Returns true if the move was blocked.
fn move_axis(world: &VoxelWorld, pos: &mut Vec3, axis: usize, delta: f32) -> bool {
    pos[axis] += delta;

    let min = *pos - PLAYER_HALF_EXTENTS;
    let max = *pos + PLAYER_HALF_EXTENTS;
    let lo = [
        min.x.floor() as i32,
        min.y.floor() as i32,
        min.z.floor() as i32,
    ];
    let hi = [
        max.x.floor() as i32,
        max.y.floor() as i32,
        max.z.floor() as i32,
    ];

    let mut collided = false;
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                if world.get_block([x, y, z]).is_air() {
                    continue;
                }
                collided = true;
                let voxel = [x, y, z];
                if delta > 0.0 {
                    pos[axis] = voxel[axis] as f32 - PLAYER_HALF_EXTENTS[axis] - SKIN;
                } else {
                    pos[axis] = (voxel[axis] + 1) as f32 + PLAYER_HALF_EXTENTS[axis] + SKIN;
                }
            }
        }
    }
    collided
}

/// World-space AABB overlap test against the player, used to block placing
/// a block inside the player.
pub fn overlaps_player(player_center: Vec3, block: [i32; 3]) -> bool {
    let min = player_center - PLAYER_HALF_EXTENTS;
    let max = player_center + PLAYER_HALF_EXTENTS;
    let bmin = Vec3::new(block[0] as f32, block[1] as f32, block[2] as f32);
    let bmax = bmin + Vec3::ONE;
    min.x < bmax.x
        && max.x > bmin.x
        && min.y < bmax.y
        && max.y > bmin.y
        && min.z < bmax.z
        && max.z > bmin.z
}
