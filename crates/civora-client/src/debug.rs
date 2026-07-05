//! Dev tooling: screenshots via F12, or automatically after a delay when
//! `CIVORA_SCREENSHOT=<path>` is set (used for scripted verification).

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

pub struct DebugPlugin;

impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (screenshot_on_f12, auto_screenshot));
    }
}

fn screenshot_on_f12(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut counter: Local<u32>,
) {
    if keys.just_pressed(KeyCode::F12) {
        let path = format!("./civora-screenshot-{}.png", *counter);
        *counter += 1;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
}

fn auto_screenshot(mut commands: Commands, time: Res<Time>, mut done: Local<bool>) {
    let delay: f32 = std::env::var("CIVORA_SCREENSHOT_DELAY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2.0);
    if *done || time.elapsed_secs() < delay {
        return;
    }
    *done = true;
    if let Ok(path) = std::env::var("CIVORA_SCREENSHOT") {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
}
