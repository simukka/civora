use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;

mod debug;
mod hud;
mod identity;
mod interact;
mod player;
mod render;
mod sim_bridge;

fn main() {
    // Unlock (or create) the player identity before the window opens: the
    // passphrase prompt is a terminal interaction.
    let player_identity = match identity::load_or_create() {
        Ok(identity) => identity,
        Err(err) => {
            eprintln!("civora: {err}");
            std::process::exit(1);
        }
    };
    println!("player id {}", player_identity.player_id());

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
        .insert_resource(identity::LocalIdentity {
            identity: player_identity,
            next_seq: 0,
        })
        .insert_resource(identity::SessionLog::default())
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
