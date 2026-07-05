//! Start screen: the Civora logo over a dark backdrop and the session
//! choice — host a world, join one over the LAN, or play offline.
//!
//! Shown only when the client launches without lobby flags; `--host` /
//! `--join` skip straight into the game (scripted runs and dedicated
//! hosts). Direct-address joins stay on the CLI (`--join <multiaddr>`);
//! the menu's join waits for mDNS discovery.

use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;

use crate::AppState;
use crate::identity::LocalIdentity;
use crate::net::{self, NetStatus};
use crate::sim_bridge::{DirtyChunks, SimWorld, generate_flat_world};

const PANEL_BG: Color = Color::srgb(0.10, 0.10, 0.15);
const BUTTON_BG: Color = Color::srgb(0.17, 0.17, 0.25);
const BUTTON_BG_HOVER: Color = Color::srgb(0.24, 0.24, 0.35);
const BUTTON_BORDER: Color = Color::srgba(1.0, 1.0, 1.0, 0.25);

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Menu), spawn_menu)
            .add_systems(OnExit(AppState::Menu), despawn_menu)
            .add_systems(
                Update,
                (hover_feedback, handle_selection).run_if(in_state(AppState::Menu)),
            );
    }
}

#[derive(Component)]
struct MenuRoot;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum MenuChoice {
    Host,
    Join,
    Offline,
}

/// The logo ships inside the binary: release artifacts are bare
/// executables, so there is no assets folder to resolve at runtime.
fn logo_image(images: &mut Assets<Image>) -> Handle<Image> {
    let image = Image::from_buffer(
        include_bytes!("../assets/logo.png"),
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::RENDER_WORLD,
    )
    .expect("bundled logo.png decodes");
    images.add(image)
}

fn spawn_menu(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let logo = logo_image(&mut images);
    commands
        .spawn((
            MenuRoot,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: px(18),
                ..default()
            },
            BackgroundColor(PANEL_BG),
            // Above the in-game HUD if states ever overlap for a frame.
            GlobalZIndex(10),
        ))
        .with_children(|parent| {
            // The logo is white-on-transparent, which is why the menu
            // panel is dark.
            parent.spawn((
                ImageNode::new(logo),
                Node {
                    width: px(520),
                    margin: UiRect::bottom(px(30)),
                    ..default()
                },
            ));

            for (choice, label) in [
                (MenuChoice::Host, "Host World  [H]"),
                (MenuChoice::Join, "Join World  [J]"),
                (MenuChoice::Offline, "Play Offline  [O]"),
            ] {
                parent
                    .spawn((
                        Button,
                        choice,
                        Node {
                            width: px(340),
                            height: px(56),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::Center,
                            border: UiRect::all(px(2)),
                            ..default()
                        },
                        BackgroundColor(BUTTON_BG),
                        BorderColor::all(BUTTON_BORDER),
                    ))
                    .with_children(|button| {
                        button.spawn((
                            Text::new(label),
                            TextFont {
                                font_size: FontSize::Px(22.0),
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        ));
                    });
            }

            parent.spawn((
                Text::new("join finds LAN worlds via mDNS; use --join <multiaddr> to dial a peer"),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.45)),
                Node {
                    margin: UiRect::top(px(26)),
                    ..default()
                },
            ));
        });
}

fn despawn_menu(mut commands: Commands, roots: Query<Entity, With<MenuRoot>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}

#[allow(clippy::type_complexity)]
fn hover_feedback(
    mut buttons: Query<(&Interaction, &mut BackgroundColor), (Changed<Interaction>, With<Button>)>,
) {
    for (interaction, mut background) in &mut buttons {
        *background = BackgroundColor(match interaction {
            Interaction::Hovered | Interaction::Pressed => BUTTON_BG_HOVER,
            Interaction::None => BUTTON_BG,
        });
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn handle_selection(
    mut commands: Commands,
    buttons: Query<(&Interaction, &MenuChoice), (Changed<Interaction>, With<Button>)>,
    keys: Res<ButtonInput<KeyCode>>,
    local: Res<LocalIdentity>,
    mut status: ResMut<NetStatus>,
    mut world: ResMut<SimWorld>,
    mut dirty: ResMut<DirtyChunks>,
    mut next: ResMut<NextState<AppState>>,
) {
    let clicked = buttons
        .iter()
        .find(|(interaction, _)| **interaction == Interaction::Pressed)
        .map(|(_, choice)| *choice);
    let choice = clicked
        .or(if keys.just_pressed(KeyCode::KeyH) {
            Some(MenuChoice::Host)
        } else if keys.just_pressed(KeyCode::KeyJ) {
            Some(MenuChoice::Join)
        } else if keys.just_pressed(KeyCode::KeyO) {
            Some(MenuChoice::Offline)
        } else {
            None
        })
        // Scripted verification (same pattern as CIVORA_SCREENSHOT): pick a
        // menu entry from the environment, exercising the selection path
        // without input automation.
        .or_else(|| match std::env::var("CIVORA_MENU").as_deref() {
            Ok("host") => Some(MenuChoice::Host),
            Ok("join") => Some(MenuChoice::Join),
            Ok("offline") => Some(MenuChoice::Offline),
            _ => None,
        });
    let Some(choice) = choice else {
        return;
    };

    match choice {
        MenuChoice::Host => {
            generate_flat_world(&mut world, &mut dirty);
            net::start_session(
                &mut commands,
                &mut status,
                local.identity.seed_bytes(),
                civora_net::SessionMode::Host,
            );
        }
        MenuChoice::Join => {
            // World stays empty; the sync delivers it (mDNS finds the host).
            net::start_session(
                &mut commands,
                &mut status,
                local.identity.seed_bytes(),
                civora_net::SessionMode::Join { dial: None },
            );
        }
        MenuChoice::Offline => generate_flat_world(&mut world, &mut dirty),
    }
    next.set(AppState::InGame);
}
