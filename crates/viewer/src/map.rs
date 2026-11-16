//! Map mode (spec §7, M1, M4).
//!
//! The map is drawn with egui: the selected layer is an egui texture with nearest filtering
//! and `Repeat` wrapping, painted on one quad whose UVs run past `[0, 1]`, so the torus repeats
//! endlessly (spec §7, "UV en fract"). The view centre stays wrapped in `[0, WORLD_SIZE)`.
//!
//! M4: climate maps (temperature, precipitation, snow cover, clouds, wind) are evaluated from
//! their annual harmonics at the chosen date, which can be animated; a day/night overlay is
//! computed per pixel from the visible sun elevation `e_vis` (spec §2.5).

use std::path::PathBuf;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use nascio_core::astro::{AstroConfig, aurora_night_factor};
use nascio_core::consts::{SECONDS_PER_DAY, WORLD_SIZE_F64};
use nascio_core::geo::{latitude, longitude, seam_distance};
use nascio_core::time::{GameClock, GameTime};
use nascio_core::torus::wrap_f64;
use nascio_worldgen::biomes;
use nascio_worldgen::preview;
use nascio_worldgen::tectonics::flags;
use nascio_worldgen::world_io::World;

/// Graticule spacing, degrees (15° of longitude = one hour of solar time).
const GRATICULE_STEP_DEG: f64 = 15.0;
/// Resolution of the day/night overlay texture (per axis, whole world).
const NIGHT_RES: usize = 256;
/// Climate textures are rebuilt when the date moves by more than this, days.
const CLIMATE_REFRESH_DAYS: f64 = 0.25;

/// What the map shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shown {
    /// A stored layer, by index.
    Layer(usize),
    /// A climate map evaluated at the current date (`preview::CLIMATE_MAPS`).
    Climate(&'static str),
}

#[derive(Resource)]
pub struct MapState {
    pub world: World,
    pub dir: PathBuf,
    shown: Shown,
    textures: Vec<Option<egui::TextureHandle>>,
    /// Climate texture and the (map, day) it was made for.
    climate_tex: Option<(egui::TextureHandle, &'static str, f64)>,
    night_tex: Option<egui::TextureHandle>,
    /// View centre, world metres.
    pub center: (f64, f64),
    /// Metres per screen point; 0 until the first frame fits the world to the view.
    mpp: f64,
    graticule: bool,
    night: bool,
    /// Displayed instant (day of year and hour of the reference longitude, continuous).
    time: GameTime,
    /// Follow the real clock.
    real_time: bool,
    /// Animation speed, game days per real second (0: paused).
    days_per_second: f64,
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
        let clock: GameClock = world.config.clock;
        Self {
            world,
            dir,
            shown: Shown::Layer(layer),
            textures,
            climate_tex: None,
            night_tex: None,
            center: (WORLD_SIZE_F64 / 2.0, WORLD_SIZE_F64 / 2.0),
            mpp: 0.0,
            graticule: true,
            night: true,
            time: clock.now(),
            real_time: true,
            days_per_second: 0.0,
            fly_request: false,
        }
    }

    /// Shows a layer or climate map by name (`--map-layer`); unknown names are ignored.
    pub fn select(&mut self, name: &str) {
        if let Some(c) = preview::CLIMATE_MAPS.iter().find(|&&c| c == name) {
            self.shown = Shown::Climate(c);
        } else if let Some(k) = self.world.layers.iter().position(|l| l.name == name) {
            self.shown = Shown::Layer(k);
        }
    }

    /// Freezes the displayed instant at day `day` of the current year, `hour` at λ = 0.
    pub fn set_date(&mut self, day: f64, hour: f64) {
        let year = self.time.year();
        self.time = GameTime::from_year_day(year, day.floor() + hour / 24.0);
        self.real_time = false;
    }

    fn has_climate(&self) -> bool {
        self.world.layer("temperature_x0").is_some()
    }

    /// Cell size of what is shown, m.
    fn cell_m(&self) -> f64 {
        match self.shown {
            Shown::Layer(k) => self.world.layers[k].cell_m,
            Shown::Climate(_) => self
                .world
                .layer("temperature_x0")
                .map_or(160.0, |l| l.cell_m),
        }
    }
}

/// Raw harmonic layers are shown through the climate maps, not listed one by one.
fn is_harmonic(name: &str) -> bool {
    ["_x0", "_a1", "_p1", "_a2", "_p2"]
        .iter()
        .any(|s| name.ends_with(s))
}

/// egui system of the map mode.
pub fn ui(mut contexts: EguiContexts, mut state: ResMut<MapState>, time: Res<Time>) -> Result {
    let ctx = contexts.ctx_mut()?;
    let state = &mut *state;
    advance_time(state, time.delta_secs_f64());
    let mut root = egui::Ui::new(
        ctx.clone(),
        "viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    egui::Panel::left("controls")
        .resizable(false)
        .min_size(230.0)
        .show(&mut root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| side_panel(ui, state));
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(&mut root, |ui| map(ui, state));
    if state.real_time || state.days_per_second != 0.0 {
        ctx.request_repaint();
    }
    Ok(())
}

fn advance_time(s: &mut MapState, dt: f64) {
    if s.real_time {
        s.time = s.world.config.clock.now();
    } else if s.days_per_second != 0.0 {
        s.time = s.time.add_seconds(dt * s.days_per_second * SECONDS_PER_DAY);
    }
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
    date_controls(ui, s);
    ui.separator();
    if s.has_climate() {
        ui.strong("Climate (at the date)");
        for name in preview::CLIMATE_MAPS {
            ui.radio_value(&mut s.shown, Shown::Climate(name), name);
        }
        ui.separator();
    }
    ui.strong("Layer");
    for (k, l) in s.world.layers.iter().enumerate() {
        if !is_harmonic(&l.name) {
            ui.radio_value(&mut s.shown, Shown::Layer(k), &l.name);
        }
    }
    ui.separator();
    ui.strong("Legend");
    let legend = match s.shown {
        Shown::Layer(k) => preview::legend(&s.world.layers[k].name),
        Shown::Climate(name) => preview::climate_legend(name),
    };
    for (color, label) in legend {
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
    ui.checkbox(&mut s.night, "Day / night (e_vis)");
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

fn date_controls(ui: &mut egui::Ui, s: &mut MapState) {
    ui.strong("Date");
    ui.label(format!(
        "year {} · day {:.1} · {} (λ = 0)",
        s.time.year(),
        s.time.day_of_year(),
        hhmm(s.time.day_frac())
    ));
    ui.checkbox(&mut s.real_time, "Real time");
    let mut day = s.time.day_of_year();
    let mut hour = s.time.day_frac() * 24.0;
    let year = s.time.year();
    let d = ui.add(egui::Slider::new(&mut day, 0.0..=364.99).text("day"));
    let h = ui.add(egui::Slider::new(&mut hour, 0.0..=23.99).text("hour"));
    if d.changed() || h.changed() {
        s.real_time = false;
        s.time = GameTime::from_year_day(year, day.floor() + hour / 24.0);
    }
    ui.horizontal(|ui| {
        ui.label("Animate");
        for (label, v) in [
            ("‖", 0.0),
            ("1 d/s", 1.0),
            ("10 d/s", 10.0),
            ("30 d/s", 30.0),
        ] {
            if ui
                .selectable_label(s.days_per_second == v && !s.real_time, label)
                .clicked()
            {
                s.days_per_second = v;
                s.real_time = false;
            }
        }
    });
    ui.horizontal(|ui| {
        for (label, n) in [
            ("Jan 15", 14.5),
            ("Apr 15", 104.5),
            ("Jul 15", 195.5),
            ("Oct 15", 287.5),
        ] {
            if ui.small_button(label).clicked() {
                s.real_time = false;
                s.time = GameTime::from_year_day(year, n);
            }
        }
    });
}

fn hhmm(day_frac: f64) -> String {
    let m = (day_frac * 24.0 * 60.0).floor() as i64;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Texture of the selected layer, created on first use (climate maps: rebuilt with the date).
fn texture(ctx: &egui::Context, s: &mut MapState) -> Option<egui::TextureId> {
    match s.shown {
        Shown::Layer(k) => {
            if s.textures[k].is_none() {
                let l = &s.world.layers[k];
                let colors = preview::colorize(&s.world, &l.name)?;
                s.textures[k] = Some(load(ctx, &l.name, l.width, l.height, &colors));
            }
            s.textures[k].as_ref().map(|t| t.id())
        }
        Shown::Climate(name) => {
            let day = s.time.day_of_year();
            let fresh = s.climate_tex.as_ref().is_some_and(|(_, n, d)| {
                *n == name && {
                    let dd = (day - d).abs();
                    dd.min(365.0 - dd) < CLIMATE_REFRESH_DAYS
                }
            });
            if !fresh {
                let base = s.world.layer("temperature_x0")?;
                let (w, h) = (base.width, base.height);
                let colors = preview::colorize_climate(&s.world, name, Some(day))?;
                s.climate_tex = Some((load(ctx, name, w, h, &colors), name, day));
            }
            s.climate_tex.as_ref().map(|(t, ..)| t.id())
        }
    }
}

fn load(
    ctx: &egui::Context,
    name: &str,
    w: usize,
    h: usize,
    colors: &[nascio_worldgen::plot::Color],
) -> egui::TextureHandle {
    let pixels = colors
        .iter()
        .map(|c| egui::Color32::from_rgb(c[0], c[1], c[2]))
        .collect();
    let image = egui::ColorImage::new([w, h], pixels);
    ctx.load_texture(name, image, egui::TextureOptions::NEAREST_REPEAT)
}

/// Night shade of a pixel from the visible sun elevation: clear by day, deepening through
/// civil, nautical and astronomical twilight (alpha in `[0, 1]`).
fn night_alpha(e_vis_deg: f64) -> f64 {
    let t = ((6.0 - e_vis_deg) / 24.0).clamp(0.0, 1.0); // +6° → 0, −18° → 1
    t * t * (3.0 - 2.0 * t)
}

/// Day/night overlay for the current instant, `NIGHT_RES²` over the whole world.
fn night_texture(ctx: &egui::Context, s: &mut MapState) -> egui::TextureId {
    let astro: AstroConfig = s.world.config.astro;
    let cell = WORLD_SIZE_F64 / NIGHT_RES as f64;
    let n = s.time.day_of_year();
    let mut pixels = Vec::with_capacity(NIGHT_RES * NIGHT_RES);
    for j in 0..NIGHT_RES {
        let z = (j as f64 + 0.5) * cell;
        let s_seam = seam_distance(z);
        for i in 0..NIGHT_RES {
            let x = (i as f64 + 0.5) * cell;
            let sun = astro.sun_at(n, s.time.hour_angle(x), z);
            let e = sun.visible_elevation.to_degrees();
            let a = night_alpha(e);
            // Aurora ring, at night only (§2.7), drawn as a green glow.
            let aurora = astro.aurora_ring(s_seam) * aurora_night_factor(sun.visible_elevation);
            let (r, g, b) = (
                (8.0 * (1.0 - aurora)) as u8,
                (12.0 + 180.0 * aurora) as u8,
                (40.0 + 60.0 * aurora) as u8,
            );
            let alpha = (a * 0.72).max(aurora * 0.5);
            pixels.push(egui::Color32::from_rgba_unmultiplied(
                r,
                g,
                b,
                (alpha * 255.0) as u8,
            ));
        }
    }
    let image = egui::ColorImage::new([NIGHT_RES, NIGHT_RES], pixels);
    let opts = egui::TextureOptions::LINEAR_REPEAT;
    match &mut s.night_tex {
        Some(t) => t.set(image, opts),
        None => s.night_tex = Some(ctx.load_texture("night", image, opts)),
    }
    s.night_tex.as_ref().expect("set above").id()
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
            let cell = s.cell_m();
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
    if let Some(tex) = texture(ui.ctx(), s) {
        painter.image(tex, rect, uv, egui::Color32::WHITE);
    }
    if s.night {
        let tex = night_texture(ui.ctx(), s);
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
    let astro: AstroConfig = s.world.config.astro;
    let sun = astro.sun(s.time, x, z);
    ui.label(format!(
        "local solar time {} · e = {:+.1}° · e_vis = {:+.1}°",
        hhmm(s.time.local_frac(x)),
        sun.elevation.to_degrees(),
        sun.visible_elevation.to_degrees()
    ));
    ui.separator();
    if s.has_climate() {
        let n = Some(s.time.day_of_year());
        let at = |name: &str| {
            let l = s.world.layer(&format!("{name}_x0"))?;
            let i = ((x / l.cell_m) as usize).min(l.width - 1);
            let j = ((z / l.cell_m) as usize).min(l.height - 1);
            preview::climate_value(&s.world, name, j * l.width + i, n)
        };
        if let (Some(t), Some(p)) = (at("temperature"), at("precipitation")) {
            ui.label(format!(
                "temperature {t:.1} °C · precipitation {p:.0} mm/yr"
            ));
        }
        if let (Some(sn), Some(c)) = (at("snow_fraction"), at("cloud_cover")) {
            ui.label(format!(
                "snow cover {:.0} % · clouds {:.0} %",
                100.0 * sn.clamp(0.0, 1.0),
                100.0 * c.clamp(0.0, 1.0)
            ));
        }
        if let (Some(u), Some(v)) = (at("wind_u"), at("wind_v")) {
            ui.label(format!("wind {:.1} m/s (u {u:+.1}, v {v:+.1})", u.hypot(v)));
        }
        ui.separator();
    }
    for l in &s.world.layers {
        if is_harmonic(&l.name) {
            continue;
        }
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
            "biome" => biomes::NAMES
                .get(v as usize)
                .copied()
                .unwrap_or("?")
                .to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_shade_is_monotonic_and_bounded() {
        assert_eq!(night_alpha(10.0), 0.0);
        assert_eq!(night_alpha(-20.0), 1.0);
        let mut prev = 0.0;
        for k in 0..100 {
            let a = night_alpha(10.0 - 0.3 * f64::from(k));
            assert!(a >= prev);
            prev = a;
        }
    }

    #[test]
    fn harmonic_layers_are_hidden() {
        assert!(is_harmonic("temperature_x0") && is_harmonic("wind_u_p2"));
        assert!(!is_harmonic("elevation") && !is_harmonic("biome"));
    }
}
