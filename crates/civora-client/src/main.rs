use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;

mod debug;
mod hud;
mod interact;
mod player;
mod render;
mod sim_bridge;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Civora — Genesis Realm".into(),
                    ..default()
                }),
                ..default()
            }),
            FrameTimeDiagnosticsPlugin::default(),
        ))
        .insert_resource(Time::<Fixed>::from_hz(20.0))
        .insert_resource(ClearColor(Color::srgb(0.55, 0.75, 0.95)))
        .add_plugins((
            sim_bridge::SimBridgePlugin,
            render::VoxelRenderPlugin,
            player::PlayerPlugin,
            interact::InteractPlugin,
            hud::HudPlugin,
            debug::DebugPlugin,
        ))
        .run();
}
