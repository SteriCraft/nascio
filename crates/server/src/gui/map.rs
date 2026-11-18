//! The world map of the server's window (moved from the viewer's map mode, spec §7).
//!
//! The map is fixed: the world square is drawn once (the torus does not repeat here) and the
//! view can zoom into it; panning stops at the edges. The "visual" aspect is shown first;
//! stored layers and climate maps at the chosen date stay available in the left panel, with a
//! day/night overlay computed from the visible sun elevation `e_vis` (spec §2.5). The players
//! are red dots; the one selected in the list is ringed and named.

use bevy::prelude::Resource;
use bevy_egui::egui;
use nascio_core::astro::{AstroConfig, aurora_night_factor};
use nascio_core::consts::{SECONDS_PER_DAY, WORLD_SIZE_F64};
use nascio_core::geo::{latitude, longitude, seam_distance};
use nascio_core::time::GameTime;
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
/// The night overlay is rebuilt when the date moves by more than this, days (about 1 min).
const NIGHT_REFRESH_DAYS: f64 = 0.0007;

/// What the map shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shown {
    /// The world as seen from above (`preview::colorize_visual`).
    Visual,
    /// A stored layer, by index.
    Layer(usize),
    /// A climate map evaluated at the current date (`preview::CLIMATE_MAPS`).
    Climate(&'static str),
}

/// A player on the map: name and position (world metres).
pub struct Dot {
    pub name: String,
    pub pos: [f64; 3],
}

#[derive(Resource)]
pub struct MapState {
    pub world: World,
    shown: Shown,
    visual_tex: Option<egui::TextureHandle>,
    textures: Vec<Option<egui::TextureHandle>>,
    /// Climate texture and the (map, day) it was made for.
    climate_tex: Option<(egui::TextureHandle, &'static str, f64)>,
    /// Night texture and the instant (days since the epoch of the clock) it was made for.
    night_tex: Option<(egui::TextureHandle, f64)>,
    /// View centre, world metres (within `[0, WORLD_SIZE]`).
    center: (f64, f64),
    /// Metres per screen point; 0 until the first frame fits the world to the view.
    mpp: f64,
    graticule: bool,
    night: bool,
    sites: bool,
    /// Displayed instant (day of year and hour of the reference longitude, continuous).
    time: GameTime,
    /// Follow the real clock.
    real_time: bool,
    /// Animation speed, game days per real second (0: paused).
    days_per_second: f64,
}

impl MapState {
    pub fn new(world: World) -> Self {
        let textures = vec![None; world.layers.len()];
        let time = world.config.clock.now();
        Self {
            world,
            shown: Shown::Visual,
            visual_tex: None,
            textures,
            climate_tex: None,
            night_tex: None,
            center: (WORLD_SIZE_F64 / 2.0, WORLD_SIZE_F64 / 2.0),
            mpp: 0.0,
            graticule: false,
            night: true,
            sites: true,
            time,
            real_time: true,
            days_per_second: 0.0,
        }
    }

    fn has_climate(&self) -> bool {
        self.world.layer("temperature_x0").is_some()
    }

    /// Cell size of what is shown, m.
    fn cell_m(&self) -> f64 {
        let named = |n: &str| self.world.layer(n).map(|l| l.cell_m);
        match self.shown {
            Shown::Visual => named("elevation")
                .or_else(|| named("elevation_tectonic"))
                .unwrap_or(160.0),
            Shown::Layer(k) => self.world.layers[k].cell_m,
            Shown::Climate(_) => named("temperature_x0").unwrap_or(160.0),
        }
    }

    /// Moves the displayed instant (real time, or the animation).
    pub fn advance_time(&mut self, dt: f64) {
        if self.real_time {
            self.time = self.world.config.clock.now();
        } else if self.days_per_second != 0.0 {
            self.time = self
                .time
                .add_seconds(dt * self.days_per_second * SECONDS_PER_DAY);
        }
    }

    /// Whether the map changes by itself (animation).
    pub fn animated(&self) -> bool {
        !self.real_time && self.days_per_second != 0.0
    }
}

/// Raw harmonic layers are shown through the climate maps, not listed one by one.
fn is_harmonic(name: &str) -> bool {
    ["_x0", "_a1", "_p1", "_a2", "_p2"]
        .iter()
        .any(|s| name.ends_with(s))
}

/// The left panel: date, aspects of the map, legend, overlays.
pub fn side_panel(ui: &mut egui::Ui, s: &mut MapState) {
    let c = &s.world.config;
    ui.heading("Carte");
    ui.label(format!(
        "graine {} · échelle 1/{} · générateur v{}",
        c.seed,
        c.scale.divisor(),
        c.generator_version
    ));
    ui.separator();
    date_controls(ui, s);
    ui.separator();
    ui.strong("Aspect");
    ui.radio_value(&mut s.shown, Shown::Visual, "Visuel");
    if s.has_climate() {
        ui.add_space(4.0);
        ui.strong("Climat (à la date)");
        for name in preview::CLIMATE_MAPS {
            ui.radio_value(&mut s.shown, Shown::Climate(name), name);
        }
    }
    ui.add_space(4.0);
    ui.strong("Couches");
    for (k, l) in s.world.layers.iter().enumerate() {
        if !is_harmonic(&l.name) {
            ui.radio_value(&mut s.shown, Shown::Layer(k), &l.name);
        }
    }
    ui.separator();
    ui.strong("Légende");
    let legend = match s.shown {
        Shown::Visual => preview::visual_legend(),
        Shown::Layer(k) => preview::world_legend(&s.world, &s.world.layers[k].name),
        Shown::Climate(name) => preview::climate_legend(name)
            .into_iter()
            .map(|(c, l)| (c, l.to_string()))
            .collect(),
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
    ui.checkbox(&mut s.night, "Jour / nuit (e_vis)");
    ui.checkbox(&mut s.sites, "Sites d'apparition");
    if s.mpp > 0.0 {
        ui.label(format!(
            "Échelle : {:.1} km pour 100 px",
            s.mpp * 100.0 / 1_000.0
        ));
    }
    if ui.button("Vue entière").clicked() {
        s.mpp = 0.0;
        s.center = (WORLD_SIZE_F64 / 2.0, WORLD_SIZE_F64 / 2.0);
    }
    ui.small("Glisser : déplacer · molette : zoomer. Les points rouges sont les joueurs.");
}

fn date_controls(ui: &mut egui::Ui, s: &mut MapState) {
    ui.strong("Date");
    ui.label(format!(
        "année {} · jour {:.1} · {} (λ = 0)",
        s.time.year(),
        s.time.day_of_year(),
        hhmm(s.time.day_frac())
    ));
    ui.checkbox(&mut s.real_time, "Temps réel");
    let mut day = s.time.day_of_year();
    let mut hour = s.time.day_frac() * 24.0;
    let year = s.time.year();
    let d = ui.add(egui::Slider::new(&mut day, 0.0..=364.99).text("jour"));
    let h = ui.add(egui::Slider::new(&mut hour, 0.0..=23.99).text("heure"));
    if d.changed() || h.changed() {
        s.real_time = false;
        s.time = GameTime::from_year_day(year, day.floor() + hour / 24.0);
    }
    ui.horizontal(|ui| {
        ui.label("Animer");
        for (label, v) in [
            ("‖", 0.0),
            ("1 j/s", 1.0),
            ("10 j/s", 10.0),
            ("30 j/s", 30.0),
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
            ("15 janv.", 14.5),
            ("15 avr.", 104.5),
            ("15 juil.", 195.5),
            ("15 oct.", 287.5),
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

/// Texture of what is shown, created on first use (climate maps: rebuilt with the date).
fn texture(ctx: &egui::Context, s: &mut MapState) -> Option<egui::TextureId> {
    match s.shown {
        Shown::Visual => {
            if s.visual_tex.is_none() {
                let l = s
                    .world
                    .layer("elevation")
                    .or_else(|| s.world.layer("elevation_tectonic"))?;
                let (w, h) = (l.width, l.height);
                let colors = preview::colorize_visual(&s.world)?;
                s.visual_tex = Some(load(ctx, "visual", w, h, &colors));
            }
            s.visual_tex.as_ref().map(|t| t.id())
        }
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
    ctx.load_texture(name, image, egui::TextureOptions::NEAREST)
}

/// Night shade of a pixel from the visible sun elevation: clear by day, deepening through
/// civil, nautical and astronomical twilight (alpha in `[0, 1]`).
fn night_alpha(e_vis_deg: f64) -> f64 {
    let t = ((6.0 - e_vis_deg) / 24.0).clamp(0.0, 1.0); // +6° → 0, −18° → 1
    t * t * (3.0 - 2.0 * t)
}

/// Day/night overlay for the current instant, `NIGHT_RES²` over the whole world (rebuilt
/// when the instant moved by more than [`NIGHT_REFRESH_DAYS`]).
fn night_texture(ctx: &egui::Context, s: &mut MapState) -> egui::TextureId {
    let stamp = s.time.day_of_year() + 365.0 * s.time.year() as f64;
    if let Some((t, at)) = &s.night_tex
        && (stamp - at).abs() < NIGHT_REFRESH_DAYS
    {
        return t.id();
    }
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
            let a = night_alpha(sun.visible_elevation.to_degrees());
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
    let opts = egui::TextureOptions::LINEAR;
    match &mut s.night_tex {
        Some((t, at)) => {
            t.set(image, opts);
            *at = stamp;
        }
        None => s.night_tex = Some((ctx.load_texture("night", image, opts), stamp)),
    }
    s.night_tex
        .as_ref()
        .map(|(t, _)| t.id())
        .expect("set above")
}

/// Keeps the view inside the world: the centre stays where the visible part does not run
/// past an edge (or in the middle when the whole axis is visible).
fn clamp_view(s: &mut MapState, rect: egui::Rect) {
    let fit = WORLD_SIZE_F64 / f64::from(rect.width().min(rect.height()).max(1.0));
    if s.mpp <= 0.0 {
        s.mpp = fit;
    }
    s.mpp = s.mpp.clamp(s.cell_m() / 32.0, fit);
    let clamp = |c: f64, extent: f32| {
        let half = f64::from(extent) * 0.5 * s.mpp;
        if 2.0 * half >= WORLD_SIZE_F64 {
            WORLD_SIZE_F64 / 2.0
        } else {
            c.clamp(half, WORLD_SIZE_F64 - half)
        }
    };
    s.center = (
        clamp(s.center.0, rect.width()),
        clamp(s.center.1, rect.height()),
    );
}

/// Screen point of world position `(x, z)`. The map is a fixed picture of `[0, WORLD_SIZE)²`
/// (it does not repeat), so this is a plain projection, not a distance on the torus.
fn to_screen(s: &MapState, rect: egui::Rect, x: f64, z: f64) -> egui::Pos2 {
    rect.center()
        + egui::vec2(
            ((x - s.center.0) / s.mpp) as f32,
            ((z - s.center.1) / s.mpp) as f32,
        )
}

/// World position under a screen point, `None` outside the world square.
fn to_world(s: &MapState, rect: egui::Rect, pos: egui::Pos2) -> Option<(f64, f64)> {
    let off = pos - rect.center();
    let (x, z) = (
        s.center.0 + f64::from(off.x) * s.mpp,
        s.center.1 + f64::from(off.y) * s.mpp,
    );
    ((0.0..WORLD_SIZE_F64).contains(&x) && (0.0..WORLD_SIZE_F64).contains(&z)).then_some((x, z))
}

/// The map, with the players (`selected`: ringed and named) and the spawn sites.
pub fn map(
    ui: &mut egui::Ui,
    s: &mut MapState,
    players: &[Dot],
    selected: Option<&str>,
    sites: &[(f64, f64)],
) {
    let (rect, response) =
        ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
    ui.painter()
        .rect_filled(rect, 0.0, egui::Color32::from_gray(24));
    clamp_view(s, rect);

    // Pan (left drag) and zoom (the point under the cursor stays put while zooming).
    if response.dragged_by(egui::PointerButton::Primary) {
        let d = response.drag_delta();
        s.center.0 -= f64::from(d.x) * s.mpp;
        s.center.1 -= f64::from(d.y) * s.mpp;
    }
    if let Some(pos) = response.hover_pos() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let off = pos - rect.center();
            let before = (
                s.center.0 + f64::from(off.x) * s.mpp,
                s.center.1 + f64::from(off.y) * s.mpp,
            );
            s.mpp *= (-f64::from(scroll) * 0.002).exp();
            clamp_view(s, rect);
            s.center = (
                before.0 - f64::from(off.x) * s.mpp,
                before.1 - f64::from(off.y) * s.mpp,
            );
        }
    }
    clamp_view(s, rect);

    let painter = ui.painter_at(rect);
    let world_rect = egui::Rect::from_min_max(
        to_screen(s, rect, 0.0, 0.0),
        to_screen(s, rect, WORLD_SIZE_F64, WORLD_SIZE_F64),
    );
    let full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    if let Some(tex) = texture(ui.ctx(), s) {
        painter.image(tex, world_rect, full, egui::Color32::WHITE);
    }
    if s.night {
        let tex = night_texture(ui.ctx(), s);
        painter.image(tex, world_rect, full, egui::Color32::WHITE);
    }
    if s.graticule {
        draw_graticule(&painter, rect, world_rect, s);
    }
    if s.sites {
        for &(x, z) in sites {
            let p = to_screen(s, rect, x, z);
            let d = 5.0;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    p + egui::vec2(0.0, -d),
                    p + egui::vec2(d, 0.0),
                    p + egui::vec2(0.0, d),
                    p + egui::vec2(-d, 0.0),
                ],
                egui::Color32::from_rgb(255, 210, 0),
                egui::Stroke::new(1.0, egui::Color32::BLACK),
            ));
        }
    }
    let red = egui::Color32::from_rgb(230, 30, 30);
    let mut chosen = None;
    for p in players {
        let at = to_screen(s, rect, p.pos[0], p.pos[2]);
        if selected == Some(p.name.as_str()) {
            chosen = Some((at, p.name.as_str()));
            continue;
        }
        painter.circle(at, 3.5, red, egui::Stroke::new(1.0, egui::Color32::BLACK));
    }
    // The selected player last, above the others.
    if let Some((at, name)) = chosen {
        painter.circle_stroke(at, 11.0, egui::Stroke::new(2.5, egui::Color32::YELLOW));
        painter.circle(at, 5.5, red, egui::Stroke::new(1.5, egui::Color32::WHITE));
        let font = egui::FontId::proportional(14.0);
        let galley = painter.layout_no_wrap(name.to_owned(), font, egui::Color32::WHITE);
        let text_rect = egui::Align2::CENTER_BOTTOM
            .anchor_size(at - egui::vec2(0.0, 15.0), galley.size())
            .expand(3.0);
        painter.rect_filled(text_rect, 3.0, egui::Color32::from_black_alpha(190));
        painter.galley(
            text_rect.min + egui::vec2(3.0, 3.0),
            galley,
            egui::Color32::WHITE,
        );
    }

    if let Some(pos) = response.hover_pos()
        && let Some((x, z)) = to_world(s, rect, pos)
    {
        response.on_hover_ui_at_pointer(|ui| tooltip(ui, s, x, z));
    }
}

fn draw_graticule(painter: &egui::Painter, rect: egui::Rect, world: egui::Rect, s: &MapState) {
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
    let steps = (180.0 / GRATICULE_STEP_DEG) as i32;
    // Parallels: z = 50 000 · (1 − sin φ), so their spacing follows sin φ (equal-area).
    for k in 0..=steps {
        let phi = -90.0 + GRATICULE_STEP_DEG * f64::from(k);
        let z = WORLD_SIZE_F64 / 2.0 * (1.0 - phi.to_radians().sin());
        let stroke = if phi == 0.0 || phi.abs() == 90.0 {
            strong
        } else {
            thin
        };
        let label = match phi {
            0.0 => "équateur".to_string(),
            90.0 => "couture ±90°".to_string(),
            -90.0 => String::new(),
            p if p > 0.0 => format!("{p:.0}°N"),
            p => format!("{:.0}°S", -p),
        };
        let y = to_screen(s, rect, 0.0, z).y;
        painter.hline(world.x_range(), y, stroke);
        painter.text(
            egui::pos2(world.left().max(rect.left()) + 4.0, y - 2.0),
            egui::Align2::LEFT_BOTTOM,
            &label,
            font.clone(),
            text,
        );
    }
    // Meridians every 15° (one hour of solar time).
    for k in 0..(360.0 / GRATICULE_STEP_DEG) as i32 {
        let lambda = GRATICULE_STEP_DEG * f64::from(k);
        let x = WORLD_SIZE_F64 * lambda / 360.0;
        let stroke = if k == 0 { strong } else { thin };
        let px = to_screen(s, rect, x, 0.0).x;
        painter.vline(px, world.y_range(), stroke);
        painter.text(
            egui::pos2(px + 3.0, world.top().max(rect.top()) + 2.0),
            egui::Align2::LEFT_TOP,
            format!("{lambda:.0}°"),
            font.clone(),
            text,
        );
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
        "heure solaire locale {} · e = {:+.1}° · e_vis = {:+.1}°",
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
                "température {t:.1} °C · précipitations {p:.0} mm/an"
            ));
        }
        if let (Some(sn), Some(c)) = (at("snow_fraction"), at("cloud_cover")) {
            ui.label(format!(
                "neige {:.0} % · nuages {:.0} %",
                100.0 * sn.clamp(0.0, 1.0),
                100.0 * c.clamp(0.0, 1.0)
            ));
        }
        if let (Some(u), Some(v)) = (at("wind_u"), at("wind_v")) {
            ui.label(format!("vent {:.1} m/s (u {u:+.1}, v {v:+.1})", u.hypot(v)));
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
