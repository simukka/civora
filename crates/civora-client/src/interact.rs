//! Block targeting and break/place. Input becomes [`Action`]s on the
//! [`ActionQueue`]; the sim applies them on the next fixed tick.

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use civora_sim::{Action, BlockId, Hit, raycast};

use crate::player::{Player, PlayerCamera, cursor_grabbed, overlaps_player};
use crate::sim_bridge::{ActionQueue, SimWorld};

const MAX_REACH: f32 = 6.0;

pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SelectedSlot>()
            .init_resource::<TargetedBlock>()
            .add_systems(Startup, spawn_highlight)
            .add_systems(
                Update,
                (select_slot, update_target, apply_clicks, update_highlight)
                    .chain()
                    .before(crate::player::cursor_grab)
                    .run_if(in_state(crate::AppState::InGame)),
            );
    }
}

/// Index into [`BlockId::PLACEABLE`], driven by number keys / scroll wheel.
#[derive(Resource, Default)]
pub struct SelectedSlot(pub usize);

impl SelectedSlot {
    pub fn block(&self) -> BlockId {
        BlockId::PLACEABLE[self.0]
    }
}

/// The block the player is currently looking at, if any.
#[derive(Resource, Default)]
pub struct TargetedBlock(pub Option<Hit>);

#[derive(Component)]
struct HighlightBox;

fn spawn_highlight(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        HighlightBox,
        Mesh3d(meshes.add(Cuboid::new(1.01, 1.01, 1.01))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 1.0, 1.0, 0.22),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        })),
        Transform::default(),
        Visibility::Hidden,
    ));
}

fn select_slot(
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    mut slot: ResMut<SelectedSlot>,
) {
    let count = BlockId::PLACEABLE.len();
    for (i, key) in [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
    ]
    .into_iter()
    .enumerate()
    {
        if keys.just_pressed(key) {
            slot.0 = i;
        }
    }
    for event in wheel.read() {
        if event.y < 0.0 {
            slot.0 = (slot.0 + 1) % count;
        } else if event.y > 0.0 {
            slot.0 = (slot.0 + count - 1) % count;
        }
    }
}

fn update_target(
    world: Res<SimWorld>,
    camera: Single<&GlobalTransform, With<PlayerCamera>>,
    mut target: ResMut<TargetedBlock>,
) {
    let origin = camera.translation();
    let dir = camera.forward();
    target.0 = raycast(
        &world.0,
        [origin.x, origin.y, origin.z],
        [dir.x, dir.y, dir.z],
        MAX_REACH,
    );
}

fn apply_clicks(
    mouse: Res<ButtonInput<MouseButton>>,
    cursor_options: Single<&bevy::window::CursorOptions>,
    target: Res<TargetedBlock>,
    slot: Res<SelectedSlot>,
    player: Single<&Transform, With<Player>>,
    net: Res<crate::net::NetStatus>,
    mut queue: ResMut<ActionQueue>,
) {
    // This runs before the grab system, so the click that captures the
    // cursor never also edits the world.
    if !cursor_grabbed(&cursor_options) || net.gate_input() {
        return;
    }
    let Some(hit) = target.0 else {
        return;
    };

    if mouse.just_pressed(MouseButton::Left) {
        queue.0.push(Action::BreakBlock { pos: hit.pos });
    }

    if mouse.just_pressed(MouseButton::Right) && hit.normal != [0, 0, 0] {
        let pos = [
            hit.pos[0] + hit.normal[0],
            hit.pos[1] + hit.normal[1],
            hit.pos[2] + hit.normal[2],
        ];
        if !overlaps_player(player.translation, pos) {
            queue.0.push(Action::PlaceBlock {
                pos,
                block: slot.block(),
            });
        }
    }
}

fn update_highlight(
    target: Res<TargetedBlock>,
    mut highlight: Single<(&mut Transform, &mut Visibility), With<HighlightBox>>,
) {
    let (transform, visibility) = &mut *highlight;
    match target.0 {
        Some(hit) => {
            transform.translation = Vec3::new(
                hit.pos[0] as f32 + 0.5,
                hit.pos[1] as f32 + 0.5,
                hit.pos[2] as f32 + 0.5,
            );
            **visibility = Visibility::Visible;
        }
        None => {
            **visibility = Visibility::Hidden;
        }
    }
}
