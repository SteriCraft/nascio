//! The client's settings, kept in `client-settings.toml` next to the executable: the account
//! (name and secret, saved after a successful connection), the last server address, and the
//! display settings of the "Paramètres" window (view distance, horizon, shadows, curvature,
//! vsync, fullscreen, faintest star, field of view) and of the game (automatic jump).
//!
//! The command line overrides them for the session without saving. Benchmarks and captures
//! ignore the file (reproducible runs).

use std::path::PathBuf;

use bevy::prelude::{Resource, warn};
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

use crate::curvature;
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
    pub shadows: bool,
    /// Display curvature on, with its radius in km.
    pub curvature: bool,
    pub curvature_radius_km: f64,
    pub vsync: bool,
    pub fullscreen: bool,
    pub star_mag_limit: f64,
    /// Vertical field of view, degrees (1.3 times wider running or flying).
    pub fov_deg: f32,
    /// Jump by itself in front of a one-block step (off by default: author's decision).
    pub auto_jump: bool,
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
            shadows: fly.shadows,
            curvature: fly.curvature_radius_m.is_some(),
            curvature_radius_km: fly.curvature_radius_m.unwrap_or(curvature::REAL_RADIUS_M)
                / 1_000.0,
            vsync: fly.vsync,
            fullscreen: false,
            star_mag_limit: fly.star_mag_limit,
            fov_deg: crate::player::FOV_DEFAULT_DEG,
            auto_jump: false,
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
        s.shadows = self.shadows;
        s.curvature_radius_m = self.curvature.then(|| {
            (self.curvature_radius_km * 1_000.0)
                .clamp(curvature::RADIUS_MIN_M, curvature::RADIUS_MAX_M)
        });
        s.vsync = self.vsync;
        s.fullscreen = self.fullscreen;
        s.star_mag_limit = self.star_mag_limit;
    }
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

/// The widgets of the "Paramètres" window. True if something changed.
pub fn settings_ui(ui: &mut egui::Ui, saved: &mut Saved) -> bool {
    let mut changed = false;
    egui::Grid::new("settings")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            ui.label("Distance de vue");
            changed |= ui
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

            ui.label("Ombres");
            changed |= ui.checkbox(&mut saved.shadows, "").changed();
            ui.end_row();

            ui.label("Courbure");
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut saved.curvature, "").changed();
                if saved.curvature {
                    changed |= ui
                        .add(
                            egui::Slider::new(
                                &mut saved.curvature_radius_km,
                                curvature::RADIUS_MIN_M / 1_000.0
                                    ..=curvature::RADIUS_MAX_M / 1_000.0,
                            )
                            .logarithmic(true)
                            .suffix(" km"),
                        )
                        .changed();
                    if ui
                        .small_button("réelle")
                        .on_hover_text("15,9 km : même circonférence que le tore")
                        .clicked()
                    {
                        saved.curvature_radius_km = curvature::REAL_RADIUS_M / 1_000.0;
                        changed = true;
                    }
                }
            });
            ui.end_row();

            ui.label("Synchronisation verticale");
            changed |= ui.checkbox(&mut saved.vsync, "").changed();
            ui.end_row();

            ui.label("Plein écran");
            changed |= ui.checkbox(&mut saved.fullscreen, "").changed();
            ui.end_row();

            ui.label("Étoiles (magnitude limite)");
            changed |= ui
                .add(
                    egui::Slider::new(
                        &mut saved.star_mag_limit,
                        crate::stars::SLIDER_BRIGHTEST..=crate::stars::FAINTEST,
                    )
                    .step_by(0.1),
                )
                .changed();
            ui.end_row();

            ui.label("Champ de vision");
            changed |= ui
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
    changed
}
