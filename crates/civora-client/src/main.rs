use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;

mod cli;
mod debug;
mod hud;
mod identity;
mod interact;
mod net;
mod player;
mod render;
mod sim_bridge;

fn main() {
    let args = match cli::parse() {
        Ok(args) => args,
        Err(err) => {
            eprintln!("civora: {err}");
            std::process::exit(2);
        }
    };

    // Unlock (or create) the player identity before the window opens: the
    // passphrase prompt is a terminal interaction.
    let player_identity = match identity::load_or_create(args.key_file) {
        Ok(identity) => identity,
        Err(err) => {
            eprintln!("civora: {err}");
            std::process::exit(1);
        }
    };
    println!("player id {}", player_identity.player_id());

    // Start networking before the window opens so the join address prints
    // to a visible terminal. Offline (no flags) spawns no thread at all.
    let joining = matches!(args.net, cli::NetMode::Join { .. });
    let net_handle = match &args.net {
        cli::NetMode::Offline => None,
        mode => {
            let session = match mode {
                cli::NetMode::Host => civora_net::SessionMode::Host,
                cli::NetMode::Join { dial } => civora_net::SessionMode::Join { dial: dial.clone() },
                cli::NetMode::Offline => unreachable!(),
            };
            Some(civora_net::spawn(civora_net::NetConfig {
                seed: player_identity.seed_bytes(),
                mode: session,
                cell: civora_net::wire::CellRef::genesis(),
                enable_mdns: true,
            }))
        }
    };

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
            sim_bridge::SimBridgePlugin {
                start_empty: joining,
            },
            net::NetPlugin {
                handle: std::sync::Mutex::new(net_handle),
                joining,
            },
            render::VoxelRenderPlugin,
            player::PlayerPlugin,
            interact::InteractPlugin,
            hud::HudPlugin,
            debug::DebugPlugin,
        ))
        .run();
}
