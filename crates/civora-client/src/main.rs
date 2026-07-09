use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;

mod cli;
mod debug;
mod hud;
mod identity;
mod interact;
mod ledger;
mod menu;
mod net;
mod player;
mod render;
mod sim_bridge;
mod voting;

/// Top-level flow: the start screen, then the world. CLI lobby flags skip
/// the menu entirely (scripted runs, muscle-memory hosts).
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Menu,
    InGame,
}

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

    // Load the accepted-proposal ledger before the window opens. A corrupt
    // ledger is a hard error naming the path (keyfile strictness): we never
    // silently drop accepted history.
    let ledger_path = match ledger::ledger_path(args.ledger_file) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("civora: {err}");
            std::process::exit(1);
        }
    };
    let accepted_ledger = match civora_governance::Ledger::load(&ledger_path) {
        Ok(ledger) => ledger,
        Err(err) => {
            eprintln!(
                "civora: cannot load ledger {}: {err}",
                ledger_path.display()
            );
            std::process::exit(1);
        }
    };
    println!(
        "ledger {} ({} accepted)",
        ledger_path.display(),
        accepted_ledger.len()
    );

    // With lobby flags, networking starts before the window opens so the
    // join address prints to a visible terminal and the menu is skipped.
    // Without flags the start screen decides, and the net thread (if any)
    // spawns on selection.
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
    let initial_state = if args.net == cli::NetMode::Offline {
        AppState::Menu
    } else {
        AppState::InGame
    };
    // The world stays empty until either the host/offline path generates it
    // or a join sync delivers it.
    let start_empty = joining || initial_state == AppState::Menu;

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
        .insert_resource(ledger::EpochClock::from_env())
        .insert_resource(ledger::LedgerStore {
            ledger: accepted_ledger,
            path: ledger_path,
        })
        .insert_state(initial_state)
        .add_plugins((
            sim_bridge::SimBridgePlugin { start_empty },
            net::NetPlugin {
                handle: std::sync::Mutex::new(net_handle),
                joining,
            },
            menu::MenuPlugin,
            render::VoxelRenderPlugin,
            player::PlayerPlugin,
            interact::InteractPlugin,
            hud::HudPlugin,
            voting::VotingPlugin,
            ledger::LedgerPlugin,
            debug::DebugPlugin,
        ))
        .run();
}
