//! The client's settings, kept in `client-settings.toml` next to the executable: the account
//! (name and secret, saved after a successful connection), the last server address, and the
//! display settings of the "Paramètres" window (view distance, horizon, transparency and its
//! distance, shadows and alpha shadows, vsync, fullscreen, faintest star, field of view) and of
//! the game (automatic jump). The display curvature is fixed (`curvature::DEFAULT_RADIUS_M`;
//! `--curvature-radius` or `--render-config` for development).
//!
//! The command line overrides them for the session without saving. Benchmarks and captures
//! ignore the file (reproducible runs).

use std::path::PathBuf;

use bevy::prelude::{Resource, warn};
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use nascio_core::consts::CHUNK_SIZE;

use crate::fly::{FlySettings, VIEW_MAX, VIEW_MIN};

/// File name, next to the executable.
const FILE: &str = "client-settings.toml";

#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// Account name.
    pub name: String,
    /// Account secret. Kept in clear in the file, like a saved password of a local game.
    pub secret: String,
    /// Last server address typed.
    pub address: String,
    pub view_distance_m: f32,
    pub horizon: bool,
    /// See-through foliage near the camera ("Transparence"), within this distance, m.
    pub transparency: bool,
    pub transparency_distance_m: f32,
    pub shadows: bool,
    /// Shadows of see-through foliage cut out too ("Ombres alpha"); off, they are solid.
    pub alpha_shadows: bool,
    pub vsync: bool,
    pub fullscreen: bool,
    pub star_mag_limit: f64,
    /// Vertical field of view, degrees (1.3 times wider running or flying).
    pub fov_deg: f32,
    /// Jump by itself in front of a one-block step (off by default: author's decision).
    pub auto_jump: bool,
    /// A predicted edit the server has not answered after this long is undone, ms.
    pub edit_timeout_ms: u32,
}

impl Default for Saved {
    fn default() -> Self {
        let fly = FlySettings::default();
        Self {
            name: String::new(),
            secret: String::new(),
            address: "127.0.0.1".into(),
            view_distance_m: fly.view_distance_m,
            horizon: fly.horizon,
            transparency: fly.foliage_cutout_chunks >= 0,
            transparency_distance_m: (fly.foliage_cutout_chunks.max(1) * CHUNK_SIZE) as f32,
            shadows: fly.shadows,
            alpha_shadows: fly.alpha_shadows,
            vsync: fly.vsync,
            fullscreen: false,
            star_mag_limit: fly.star_mag_limit,
            fov_deg: crate::player::FOV_DEFAULT_DEG,
            auto_jump: false,
            edit_timeout_ms: 1000,
        }
    }
}

impl Saved {
    /// `client-settings.toml` next to the executable (the working directory if unknown).
    pub fn path() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_default()
            .join(FILE)
    }

    /// The saved settings, or the defaults if the file is missing or unreadable.
    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                warn!("{}: {e}; default settings", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = Self::path();
        let written = toml::to_string(self)
            .map_err(anyhow::Error::from)
            .and_then(|s| std::fs::write(&path, s).map_err(anyhow::Error::from));
        if let Err(e) = written {
            warn!("cannot save {}: {e:#}", path.display());
        }
    }

    /// Copies the display settings into the 3D view's.
    pub fn apply(&self, s: &mut FlySettings) {
        s.view_distance_m = self.view_distance_m.clamp(VIEW_MIN, VIEW_MAX);
        s.horizon = self.horizon;
        s.foliage_cutout_chunks = if self.transparency {
            cutout_chunks(self.transparency_distance_m)
        } else {
            -1
        };
        s.shadows = self.shadows;
        s.alpha_shadows = self.alpha_shadows;
        s.vsync = self.vsync;
        s.fullscreen = self.fullscreen;
        s.star_mag_limit = self.star_mag_limit;
    }
}

/// Range of the "Distance de transparence" slider, m (one chunk to the view distance).
const TRANSPARENCY_MIN: f32 = CHUNK_SIZE as f32;

/// Cut-out radius, chunk columns, of a transparency distance in m.
fn cutout_chunks(distance_m: f32) -> i32 {
    (distance_m.clamp(TRANSPARENCY_MIN, VIEW_MAX) / CHUNK_SIZE as f32).round() as i32
}

/// The "Paramètres" window, shown while `open`: a change applies at once, closing it (cross
/// or "Fermer") saves the file.
pub fn settings_window(
    ctx: &egui::Context,
    open: &mut bool,
    saved: &mut Saved,
    fly: &mut FlySettings,
) {
    if !*open {
        return;
    }
    let mut close = false;
    crate::theme::dialog(
        ctx,
        "settings",
        Some("Paramètres"),
        [0.0, 0.0],
        Some(&mut *open),
        |ui| {
            if settings_ui(ui, saved) {
                saved.apply(fly);
            }
            ui.add_space(8.0);
            close = ui.button("Fermer").clicked();
        },
    );
    if close {
        *open = false;
    }
    if !*open {
        saved.save();
    }
}

/// Width of the cell of a slider and its value, px: fixed, so that the window does not
/// change width as the value's text does.
const SLIDER_CELL: f32 = 200.0;

/// A cell of [`SLIDER_CELL`] px for one widget, left aligned.
fn fixed(ui: &mut egui::Ui) -> egui::Ui {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(SLIDER_CELL, ui.spacing().interact_size.y),
        egui::Sense::hover(),
    );
    ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    )
}

/// The widgets of the "Paramètres" window, in two columns. True if something changed.
pub fn settings_ui(ui: &mut egui::Ui, saved: &mut Saved) -> bool {
    let mut changed = false;
    ui.spacing_mut().slider_width = 110.0;
    ui.horizontal_top(|ui| {
        // Left: view and window, game.
        egui::Grid::new("settings_left")
            .num_columns(2)
            .spacing([16.0, 10.0])
            .show(ui, |ui| {
                ui.label("Distance de vue");
                changed |= fixed(ui)
                    .add(
                        egui::Slider::new(&mut saved.view_distance_m, VIEW_MIN..=VIEW_MAX)
                            .suffix(" m")
                            .step_by(32.0),
                    )
                    .changed();
                ui.end_row();

                ui.label("Horizon lointain");
                changed |= ui.checkbox(&mut saved.horizon, "").changed();
                ui.end_row();

                ui.label("Plein écran");
                changed |= ui.checkbox(&mut saved.fullscreen, "").changed();
                ui.end_row();

                ui.label("Synchronisation verticale");
                changed |= ui.checkbox(&mut saved.vsync, "").changed();
                ui.end_row();

                ui.label("Champ de vision");
                changed |= fixed(ui)
                    .add(
                        egui::Slider::new(
                            &mut saved.fov_deg,
                            crate::player::FOV_MIN_DEG..=crate::player::FOV_MAX_DEG,
                        )
                        .suffix("°")
                        .step_by(1.0),
                    )
                    .on_hover_text("Vertical ; 1,3 fois plus large en course et en vol")
                    .changed();
                ui.end_row();

                ui.label("Saut automatique");
                changed |= ui
                    .checkbox(&mut saved.auto_jump, "")
                    .on_hover_text("Saute seul devant une marche d'un bloc")
                    .changed();
                ui.end_row();
            });
        ui.add_space(32.0);
        // Right: shadows, transparency, sky.
        egui::Grid::new("settings_right")
            .num_columns(2)
            .spacing([16.0, 10.0])
            .show(ui, |ui| {
                ui.label("Ombres");
                changed |= ui.checkbox(&mut saved.shadows, "").changed();
                ui.end_row();

                ui.label("Ombres alpha");
                changed |= ui
                    .add_enabled(
                        saved.shadows && saved.transparency,
                        egui::Checkbox::new(&mut saved.alpha_shadows, ""),
                    )
                    .on_hover_text(
                        "Désactivées, le feuillage ajouré projette des ombres pleines (plus rapide)",
                    )
                    .changed();
                ui.end_row();

                ui.label("Transparence");
                changed |= ui
                    .checkbox(&mut saved.transparency, "")
                    .on_hover_text("Feuillage ajouré près de la caméra ; opaque au-delà")
                    .changed();
                ui.end_row();

                ui.label("Distance de transparence");
                changed |= fixed(ui)
                    .add_enabled(
                        saved.transparency,
                        egui::Slider::new(
                            &mut saved.transparency_distance_m,
                            TRANSPARENCY_MIN..=VIEW_MAX,
                        )
                        .suffix(" m")
                        .step_by(CHUNK_SIZE as f64),
                    )
                    .changed();
                ui.end_row();

                ui.label("Étoiles (magnitude limite)");
                changed |= fixed(ui)
                    .add(
                        egui::Slider::new(
                            &mut saved.star_mag_limit,
                            crate::stars::SLIDER_BRIGHTEST..=crate::stars::FAINTEST,
                        )
                        .step_by(0.1),
                    )
                    .changed();
                ui.end_row();
            });
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparency_settings_reach_the_view() {
        let mut fly = FlySettings::default();
        let mut saved = Saved::default();
        // The defaults round-trip: 4 chunks, alpha shadows on.
        saved.apply(&mut fly);
        assert_eq!(
            fly.foliage_cutout_chunks,
            crate::curvature::DEFAULT_FOLIAGE_CUTOUT_CHUNKS
        );
        assert!(fly.alpha_shadows);
        saved.transparency_distance_m = 200.0;
        saved.alpha_shadows = false;
        saved.apply(&mut fly);
        assert_eq!(fly.foliage_cutout_chunks, 6);
        assert!(!fly.alpha_shadows);
        // Off: no cut-out foliage at all.
        saved.transparency = false;
        saved.apply(&mut fly);
        assert!(fly.foliage_cutout_chunks < 0);
        // Out of range distances are clamped.
        assert_eq!(cutout_chunks(0.0), 1);
        assert_eq!(cutout_chunks(1e6), (VIEW_MAX / CHUNK_SIZE as f32) as i32);
    }
}
