//! Map mode (spec §7, M1).
//!
//! The map is drawn with egui: the selected layer is an egui texture with nearest filtering
//! and `Repeat` wrapping, painted on one quad whose UVs run past `[0, 1]`, so the torus repeats
//! endlessly (spec §7, "UV en fract"). The view centre stays wrapped in `[0, WORLD_SIZE)`.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use nascio_core::consts::WORLD_SIZE_F64;
use nascio_core::geo::{latitude, longitude, seam_distance};
use nascio_core::torus::wrap_f64;
use nascio_worldgen::preview;
use nascio_worldgen::tectonics::flags;
use nascio_worldgen::world_io::World;

/// Graticule spacing, degrees (15° of longitude = one hour of solar time).
const GRATICULE_STEP_DEG: f64 = 15.0;

#[derive(Resource)]
pub struct MapState {
    pub world: World,
    pub dir: PathBuf,
    layer: usize,
    textures: Vec<Option<egui::TextureHandle>>,
    /// View centre, world metres.
    pub center: (f64, f64),
    /// Metres per screen point; 0 until the first frame fits the world to the view.
    mpp: f64,
    graticule: bool,
    /// Set by the "Fly here" button: switch to 3D at the view centre.
    pub fly_request: bool,
}

impl MapState {
    pub fn new(world: World, dir: PathBuf) -> Self {
        let layer = world
            .layers
            .iter()
            .position(|l| l.name == "elevation_tectonic")
            .unwrap_or(0);
        let textures = vec![None; world.layers.len()];
        Self {
            world,
            dir,
            layer,
            textures,
            center: (WORLD_SIZE_F64 / 2.0, WORLD_SIZE_F64 / 2.0),
            mpp: 0.0,
            graticule: true,
            fly_request: false,
        }
    }
}

/// egui system of the map mode.
pub fn ui(mut contexts: EguiContexts, mut state: ResMut<MapState>) -> Result {
    let ctx = contexts.ctx_mut()?;
    let state = &mut *state;
    let mut root = egui::Ui::new(
        ctx.clone(),
        "viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    egui::Panel::left("controls")
        .resizable(false)
        .min_size(220.0)
        .show(&mut root, |ui| side_panel(ui, state));
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(&mut root, |ui| map(ui, state));
    Ok(())
}

fn side_panel(ui: &mut egui::Ui, s: &mut MapState) {
    ui.heading("Nascio — map");
    let c = &s.world.config;
    ui.label(format!(
        "{}\nseed {} · scale 1/{} · generator v{}",
        s.dir.display(),
        c.seed,
        c.scale.divisor(),
        c.generator_version
    ));
    ui.separator();
    ui.strong("Layer");
    for (k, l) in s.world.layers.iter().enumerate() {
        ui.radio_value(&mut s.layer, k, &l.name);
    }
    ui.separator();
    ui.strong("Legend");
    for (color, label) in preview::legend(&s.world.layers[s.layer].name) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 10.0), egui::Sense::hover());
            ui.painter().rect_filled(
                r,
                0.0,
                egui::Color32::from_rgb(color[0], color[1], color[2]),
            );
            ui.label(label);
        });
    }
    ui.separator();
    ui.checkbox(&mut s.graticule, "Graticule (15°)");
    if s.mpp > 0.0 {
        ui.label(format!(
            "Scale: {:.1} km per 100 px",
            s.mpp * 100.0 / 1_000.0
        ));
    }
    if ui.button("Fly here (3D)").clicked() {
        s.fly_request = true;
    }
    if ui.button("Reset view").clicked() {
        s.center = (WORLD_SIZE_F64 / 2.0, WORLD_SIZE_F64 / 2.0);
        s.mpp = 0.0;
    }
    ui.separator();
    ui.small("Drag to pan, wheel to zoom. The map repeats: the world is a torus.");
}

/// Texture of the selected layer, created on first use.
fn texture(ctx: &egui::Context, s: &mut MapState) -> Option<egui::TextureId> {
    let k = s.layer;
    if s.textures[k].is_none() {
        let l = &s.world.layers[k];
        let colors = preview::colorize(&s.world, &l.name)?;
        let pixels = colors
            .iter()
            .map(|c| egui::Color32::from_rgb(c[0], c[1], c[2]))
            .collect();
        let image = egui::ColorImage::new([l.width, l.height], pixels);
        s.textures[k] =
            Some(ctx.load_texture(l.name.clone(), image, egui::TextureOptions::NEAREST_REPEAT));
    }
    s.textures[k].as_ref().map(|t| t.id())
}

fn map(ui: &mut egui::Ui, s: &mut MapState) {
    let (rect, response) =
        ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    if s.mpp <= 0.0 {
        s.mpp = WORLD_SIZE_F64 / f64::from(rect.width().min(rect.height()).max(1.0));
    }

    // Pan and zoom (the point under the cursor stays put while zooming).
    if response.dragged() {
        let d = response.drag_delta();
        s.center.0 = wrap_f64(s.center.0 - f64::from(d.x) * s.mpp);
        s.center.1 = wrap_f64(s.center.1 - f64::from(d.y) * s.mpp);
    }
    if let Some(pos) = response.hover_pos() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let before = screen_to_world(s, rect, pos);
            let cell = s.world.layers[s.layer].cell_m;
            let max_mpp = 4.0 * WORLD_SIZE_F64 / f64::from(rect.width().max(1.0));
            s.mpp = (s.mpp * (-f64::from(scroll) * 0.002).exp()).clamp(cell / 32.0, max_mpp);
            let off = pos - rect.center();
            s.center = (
                wrap_f64(before.0 - f64::from(off.x) * s.mpp),
                wrap_f64(before.1 - f64::from(off.y) * s.mpp),
            );
        }
    }

    let painter = ui.painter_at(rect);
    if let Some(tex) = texture(ui.ctx(), s) {
        let half = (
            f64::from(rect.width()) * 0.5 * s.mpp,
            f64::from(rect.height()) * 0.5 * s.mpp,
        );
        let uv = egui::Rect::from_min_max(
            egui::pos2(
                ((s.center.0 - half.0) / WORLD_SIZE_F64) as f32,
                ((s.center.1 - half.1) / WORLD_SIZE_F64) as f32,
            ),
            egui::pos2(
                ((s.center.0 + half.0) / WORLD_SIZE_F64) as f32,
                ((s.center.1 + half.1) / WORLD_SIZE_F64) as f32,
            ),
        );
        painter.image(tex, rect, uv, egui::Color32::WHITE);
    }
    if s.graticule {
        draw_graticule(&painter, rect, s);
    }

    if let Some(pos) = response.hover_pos() {
        let (x, z) = screen_to_world(s, rect, pos);
        response.on_hover_ui_at_pointer(|ui| tooltip(ui, s, x, z));
    }
}

fn screen_to_world(s: &MapState, rect: egui::Rect, pos: egui::Pos2) -> (f64, f64) {
    let off = pos - rect.center();
    (
        wrap_f64(s.center.0 + f64::from(off.x) * s.mpp),
        wrap_f64(s.center.1 + f64::from(off.y) * s.mpp),
    )
}

/// Screen positions of every repetition of world coordinate `v` along one axis.
fn repeats(v: f64, center: f64, mpp: f64, screen_center: f32, half_extent: f32) -> Vec<f32> {
    let half_m = f64::from(half_extent) * mpp;
    let first = ((center - half_m - v) / WORLD_SIZE_F64).ceil() as i64;
    let last = ((center + half_m - v) / WORLD_SIZE_F64).floor() as i64;
    (first..=last)
        .map(|k| screen_center + ((v + k as f64 * WORLD_SIZE_F64 - center) / mpp) as f32)
        .collect()
}

fn draw_graticule(painter: &egui::Painter, rect: egui::Rect, s: &MapState) {
    let thin = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 90),
    );
    let strong = egui::Stroke::new(
        1.5,
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 190),
    );
    let text = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 220);
    let font = egui::FontId::proportional(11.0);
    let c = rect.center();
    let steps = (180.0 / GRATICULE_STEP_DEG) as i32;
    // Parallels: z = 50 000 · (1 − sin φ), so their spacing follows sin φ (equal-area).
    for k in 0..=steps {
        let phi = -90.0 + GRATICULE_STEP_DEG * f64::from(k);
        if phi == -90.0 {
            continue; // same line as +90° (the seam)
        }
        let z = WORLD_SIZE_F64 / 2.0 * (1.0 - phi.to_radians().sin());
        let stroke = if phi == 0.0 || phi == 90.0 {
            strong
        } else {
            thin
        };
        let label = match phi {
            0.0 => "equator".to_string(),
            90.0 => "seam ±90°".to_string(),
            p if p > 0.0 => format!("{p:.0}°N"),
            p => format!("{:.0}°S", -p),
        };
        for y in repeats(z, s.center.1, s.mpp, c.y, rect.height() * 0.5) {
            painter.hline(rect.x_range(), y, stroke);
            painter.text(
                egui::pos2(rect.left() + 4.0, y - 2.0),
                egui::Align2::LEFT_BOTTOM,
                &label,
                font.clone(),
                text,
            );
        }
    }
    // Meridians every 15° (one hour of solar time).
    for k in 0..(360.0 / GRATICULE_STEP_DEG) as i32 {
        let lambda = GRATICULE_STEP_DEG * f64::from(k);
        let x = WORLD_SIZE_F64 * lambda / 360.0;
        let stroke = if k == 0 { strong } else { thin };
        for px in repeats(x, s.center.0, s.mpp, c.x, rect.width() * 0.5) {
            painter.vline(px, rect.y_range(), stroke);
            painter.text(
                egui::pos2(px + 3.0, rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                format!("{lambda:.0}°"),
                font.clone(),
                text,
            );
        }
    }
}

fn tooltip(ui: &mut egui::Ui, s: &MapState, x: f64, z: f64) {
    ui.label(format!("x = {x:.0} m   z = {z:.0} m"));
    ui.label(format!(
        "φ = {:+.2}°   λ = {:.2}°   s = {:.0} m",
        latitude(z).to_degrees(),
        longitude(x).to_degrees(),
        seam_distance(z)
    ));
    ui.separator();
    for l in &s.world.layers {
        let i = ((x / l.cell_m) as usize).min(l.width - 1);
        let j = ((z / l.cell_m) as usize).min(l.height - 1);
        let v = l.value(i, j);
        let shown = match l.name.as_str() {
            "crust_type" => if v as u8 == 1 {
                "continental"
            } else {
                "oceanic"
            }
            .to_string(),
            "history_flags" => flag_names(v as u8),
            "hotspot_track" if v < 0.0 => "—".to_string(),
            "plate_id" => format!("{v:.0}"),
            _ => format!("{v:.0} {}", l.unit),
        };
        ui.label(format!("{}: {shown}", l.name));
    }
}

fn flag_names(f: u8) -> String {
    let names = [
        (flags::OROGENY, "orogeny"),
        (flags::ARC, "arc"),
        (flags::RIFT, "rift"),
        (flags::HOTSPOT, "hotspot"),
        (flags::OPHIOLITE, "ophiolite"),
        (flags::TRENCH, "trench"),
        (flags::RIDGE, "ridge"),
    ];
    let v: Vec<&str> = names
        .iter()
        .filter(|(b, _)| f & b != 0)
        .map(|(_, n)| *n)
        .collect();
    if v.is_empty() {
        "—".into()
    } else {
        v.join(", ")
    }
}
