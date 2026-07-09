//! HUD: crosshair, hotbar, and debug overlay.

use std::fmt::Write;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use civora_sim::BlockId;

use crate::interact::{SelectedSlot, TargetedBlock};
use crate::player::Player;
use crate::render::block_color;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        // The HUD belongs to the world, not the start screen.
        app.add_systems(OnEnter(crate::AppState::InGame), spawn_hud)
            .add_systems(
                Update,
                (update_hotbar_selection, update_debug_text)
                    .run_if(in_state(crate::AppState::InGame)),
            );
    }
}

#[derive(Component)]
struct HotbarSlotUi(usize);

#[derive(Component)]
struct DebugText;

fn spawn_hud(mut commands: Commands, slot: Res<SelectedSlot>) {
    // Crosshair: two thin bars centered on screen.
    for (width, height) in [(12.0, 2.0), (2.0, 12.0)] {
        commands.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: percent(50),
                top: percent(50),
                width: px(width),
                height: px(height),
                margin: UiRect {
                    left: px(-width / 2.0),
                    top: px(-height / 2.0),
                    ..default()
                },
                ..default()
            },
            BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.8)),
        ));
    }

    // Hotbar: one colored slot per placeable block, centered at the bottom.
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            bottom: px(16),
            width: percent(100),
            justify_content: JustifyContent::Center,
            column_gap: px(6),
            ..default()
        })
        .with_children(|parent| {
            for (i, block) in BlockId::PLACEABLE.into_iter().enumerate() {
                let [r, g, b] = block_color(block);
                // The HUD can spawn mid-session (after the start screen),
                // so the current selection is applied here, not only by the
                // change-detection system.
                let border = if i == slot.0 {
                    Color::WHITE
                } else {
                    Color::srgba(0.0, 0.0, 0.0, 0.6)
                };
                parent.spawn((
                    HotbarSlotUi(i),
                    Node {
                        width: px(44),
                        height: px(44),
                        border: UiRect::all(px(3)),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(r, g, b)),
                    BorderColor::all(border),
                ));
            }
        });

    // Debug overlay.
    commands.spawn((
        DebugText,
        Text::default(),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: px(8),
            left: px(8),
            ..default()
        },
    ));
}

fn update_hotbar_selection(
    slot: Res<SelectedSlot>,
    mut slots: Query<(&HotbarSlotUi, &mut BorderColor)>,
) {
    if !slot.is_changed() {
        return;
    }
    for (ui, mut border) in &mut slots {
        *border = if ui.0 == slot.0 {
            BorderColor::all(Color::WHITE)
        } else {
            BorderColor::all(Color::srgba(0.0, 0.0, 0.0, 0.6))
        };
    }
}

#[allow(clippy::too_many_arguments)]
fn update_debug_text(
    diagnostics: Res<DiagnosticsStore>,
    player: Single<(&Player, &Transform)>,
    target: Res<TargetedBlock>,
    world: Res<crate::sim_bridge::SimWorld>,
    slot: Res<SelectedSlot>,
    local: Res<crate::identity::LocalIdentity>,
    log: Res<crate::identity::SessionLog>,
    net: Res<crate::net::NetStatus>,
    roster: Res<crate::net::PeerRoster>,
    store: Res<crate::voting::ProposalStore>,
    mut text: Single<&mut Text, With<DebugText>>,
) {
    let (player, transform) = *player;
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);
    let pos = transform.translation;
    let targeted = match target.0 {
        Some(hit) => format!("{} {:?}", world.0.get_block(hit.pos).name(), hit.pos),
        None => "none".to_string(),
    };

    let text = &mut text.0;
    text.clear();
    let _ = writeln!(text, "fps {fps:.0}");
    let _ = writeln!(
        text,
        "id {} ({} signed actions)",
        local.identity.player_id().short(),
        log.0.len()
    );
    let _ = writeln!(text, "pos {:.1} {:.1} {:.1}", pos.x, pos.y, pos.z);
    let _ = writeln!(
        text,
        "mode {}",
        if player.flying { "fly (F)" } else { "walk (F)" }
    );
    let _ = writeln!(text, "target {targeted}");
    let _ = writeln!(text, "hand {}", slot.block().name());

    use crate::net::NetPhase;
    let phase = match net.phase {
        NetPhase::Offline => "offline",
        NetPhase::Host => "host",
        NetPhase::Joining => "joining...",
        NetPhase::Live => "live",
    };
    // ASCII only: the default font has no em-dash glyph.
    let _ = writeln!(text, "net {phase}, {} peer(s)", roster.0.len());
    for (player, addr) in &roster.0 {
        let _ = writeln!(text, "  peer {} {}", player.short(), addr);
    }
    if net.diverged {
        let _ = writeln!(text, "DIVERGED - resyncing");
    }
    if let Some(err) = &net.last_error {
        let _ = writeln!(text, "net error: {err}");
    }

    let _ = writeln!(text, "proposals: {} open (P)", store.open_count());

    let _ = writeln!(text, "click to grab cursor, Esc to release");
}
