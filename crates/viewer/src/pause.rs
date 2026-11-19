//! The in-game menu: Échap (chat closed, god mode off) opens "Reprendre / Paramètres /
//! Quitter"; Échap again, or "Reprendre", goes back to the game. While it is open the mouse is
//! free and the keyboard does not move the camera.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::fly::FlySettings;
use crate::settings::Saved;

#[derive(Resource, Default)]
pub struct Pause {
    pub open: bool,
    settings: bool,
}

impl Pause {
    fn close(&mut self, saved: &Saved) {
        if self.settings {
            saved.save();
        }
        self.open = false;
        self.settings = false;
    }
}

/// Échap toggles the menu. Runs before the god mode and the chat see the key: the chat open or
/// the god mode on, Échap is theirs (closing the chat, leaving the god mode).
pub fn pause_input(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<crate::chat::ChatBox>,
    god: Res<crate::god::God>,
    cam: Res<crate::fly::FlyCam>,
    saved: Res<Saved>,
    mut pause: ResMut<Pause>,
) {
    if cam.scripted || chat.open || god.on || !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    if pause.open {
        pause.close(&saved);
    } else {
        pause.open = true;
    }
}

pub fn pause_ui(
    mut contexts: EguiContexts,
    mut pause: ResMut<Pause>,
    mut saved: ResMut<Saved>,
    mut settings: ResMut<FlySettings>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    if !pause.open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let pause = &mut *pause;
    // Dim the game behind.
    egui::Area::new(egui::Id::new("pause-dim"))
        .order(egui::Order::Background)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            ui.painter().rect_filled(
                ctx.viewport_rect(),
                0.0,
                egui::Color32::from_black_alpha(110),
            );
        });
    if pause.settings {
        crate::settings::settings_window(ctx, &mut pause.settings, &mut saved, &mut settings);
        return Ok(());
    }
    let mut resume = false;
    crate::theme::dialog(ctx, "pause", None, [0.0, 0.0], None, |ui| {
        ui.vertical_centered(|ui| {
            // The main menu's spacing: centres 70 px apart.
            let gap = crate::theme::BUTTON_STEP - crate::theme::BUTTON.y;
            ui.spacing_mut().item_spacing.y = gap;
            let big = |t| crate::theme::big_button(t, 1.0);
            resume = ui.add(big("Reprendre")).clicked();
            if ui.add(big("Paramètres")).clicked() {
                pause.settings = true;
            }
            if ui.add(big("Quitter")).clicked() {
                exit.write(AppExit::Success);
            }
        });
    });
    if resume {
        pause.close(&saved);
    }
    Ok(())
}
