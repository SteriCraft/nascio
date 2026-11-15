//! Nascio development viewer. Minimal window for M0; map mode arrives in M1.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Nascio viewer".into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .add_systems(Startup, setup)
        .add_systems(EguiPrimaryContextPass, ui)
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn ui(mut contexts: EguiContexts) -> Result {
    egui::Window::new("Nascio").show(contexts.ctx_mut()?, |ui| {
        ui.label(format!("World size: {} m", nascio_core::consts::WORLD_SIZE));
    });
    Ok(())
}
