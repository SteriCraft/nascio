//! Layer colouring and PNG previews (`previews/*.png`).
//!
//! [`colorize`] and [`legend`] work on the layers of a loaded [`World`], so the CLI previews
//! and the viewer share the same palettes.

use std::f64::consts::TAU;
use std::path::{Path, PathBuf};

use image::{Rgb, RgbImage};
use nascio_core::consts::WORLD_SIZE_F64;

use crate::grid::PeriodicGrid;
use crate::plot::{Canvas, Color, hex, text_height, text_width};
use crate::tectonics::{CONTINENTAL, Plate, flags};
use crate::world_io::{Layer, LayerData, World};

/// Categorical order (dataviz reference palette, light mode).
const SERIES: [&str; 8] = [
    "#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948",
];
/// Sequential blue ramp, light to dark.
const BLUES: [&str; 7] = [
    "#cde2fb", "#9ec5f4", "#6da7ec", "#3987e5", "#256abf", "#184f95", "#0d366b",
];
const SURFACE: &str = "#fcfcfb";
const INK: &str = "#0b0b0b";
const INK_2: &str = "#52514e";
const CONTINENT_GREY: &str = "#c9c6bd";
const OCEAN_FILL: &str = "#2f6db0";
const CONTINENT_FILL: &str = "#c9a66b";

/// Hypsometric tint, metres → colour.
const ELEVATION_STOPS: [(f64, &str); 11] = [
    (-3_500.0, "#0d2a4f"),
    (-3_000.0, "#13396b"),
    (-1_500.0, "#2f6db0"),
    (-200.0, "#6fa8dc"),
    (-0.001, "#a9d0ef"),
    (0.0, "#5f9e4f"),
    (400.0, "#9cba68"),
    (1_000.0, "#cdb877"),
    (1_800.0, "#a0764a"),
    (2_500.0, "#7d5a43"),
    (3_000.0, "#f4f2ee"),
];

/// History flags in drawing priority (first match wins).
const FLAG_ORDER: [(u8, &str, &str); 7] = [
    (flags::HOTSPOT, SERIES[3], "hotspot"),
    (flags::ARC, SERIES[7], "arc"),
    (flags::TRENCH, SERIES[6], "trench"),
    (flags::OROGENY, SERIES[1], "orogeny"),
    (flags::OPHIOLITE, SERIES[4], "ophiolite"),
    (flags::RIFT, SERIES[2], "rift"),
    (flags::RIDGE, SERIES[0], "ridge"),
];

/// Age at which the hotspot-track colour saturates, Ma.
const HOTSPOT_AGE_SCALE: f64 = 120.0;

fn lerp_color(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let c = |k: usize| (f64::from(a[k]) + (f64::from(b[k]) - f64::from(a[k])) * t).round() as u8;
    Rgb([c(0), c(1), c(2)])
}

fn ramp(stops: &[(f64, &str)], v: f64) -> Color {
    if v <= stops[0].0 {
        return hex(stops[0].1);
    }
    for w in stops.windows(2) {
        if v <= w[1].0 {
            return lerp_color(hex(w[0].1), hex(w[1].1), (v - w[0].0) / (w[1].0 - w[0].0));
        }
    }
    hex(stops[stops.len() - 1].1)
}

/// Sequential blue for `t ∈ [0, 1]`.
fn blues(t: f64) -> Color {
    let t = t.clamp(0.0, 1.0) * (BLUES.len() - 1) as f64;
    let k = (t.floor() as usize).min(BLUES.len() - 2);
    lerp_color(hex(BLUES[k]), hex(BLUES[k + 1]), t - k as f64)
}

/// Hypsometric colour of an elevation.
pub fn elevation_color(e: f64) -> Color {
    ramp(&ELEVATION_STOPS, e)
}

/// Desaturated, lightened elevation used under overlays.
fn faded_elevation(e: f64) -> Color {
    let c = elevation_color(e);
    let grey = (0.3 * f64::from(c[0]) + 0.59 * f64::from(c[1]) + 0.11 * f64::from(c[2])) as u8;
    lerp_color(Rgb([grey, grey, grey]), hex(SURFACE), 0.45)
}

fn u8_grid(l: &Layer) -> Option<PeriodicGrid<u8>> {
    match &l.data {
        LayerData::U8(v) => Some(PeriodicGrid::from_vec(
            l.width,
            l.height,
            l.cell_m,
            v.clone(),
        )),
        _ => None,
    }
}

/// Greedy colouring of the plate adjacency graph with the 8 categorical colours, so
/// neighbouring plates never share a colour (falls back to cycling if impossible).
fn plate_colors(plate_id: &PeriodicGrid<u8>) -> Vec<Color> {
    let n = plate_id
        .data()
        .iter()
        .copied()
        .max()
        .map_or(0, |m| m as usize + 1);
    let mut adj = vec![vec![false; n]; n];
    for idx in 0..plate_id.data().len() {
        let (i, j) = plate_id.coords(idx);
        let a = plate_id.data()[idx] as usize;
        for (di, dj) in [(1i64, 0i64), (0, 1)] {
            let b = *plate_id.get(i as i64 + di, j as i64 + dj) as usize;
            if a != b {
                adj[a][b] = true;
                adj[b][a] = true;
            }
        }
    }
    let mut slot = vec![usize::MAX; n];
    for p in 0..n {
        let used: Vec<usize> = (0..n).filter(|&q| adj[p][q]).map(|q| slot[q]).collect();
        slot[p] = (0..SERIES.len())
            .find(|s| !used.contains(s))
            .unwrap_or(p % SERIES.len());
    }
    slot.into_iter().map(|s| hex(SERIES[s])).collect()
}

/// Per-cell colours of a layer (row-major), or `None` for an unknown layer.
pub fn colorize(world: &World, name: &str) -> Option<Vec<Color>> {
    let layer = world.layer(name)?;
    let n = layer.width * layer.height;
    let elev = |i: usize| {
        world
            .layer("elevation_tectonic")
            .map_or(0.0, |l| l.data.get_f64(i))
    };
    let continental = |i: usize| {
        world
            .layer("crust_type")
            .is_some_and(|l| l.data.get_f64(i) as u8 == CONTINENTAL)
    };
    let v = |i: usize| layer.data.get_f64(i);
    let colors = match name {
        "elevation_tectonic" => (0..n).map(|i| elevation_color(v(i))).collect(),
        "plate_id" => {
            let palette = plate_colors(&u8_grid(layer)?);
            (0..n)
                .map(|i| {
                    let c = palette[v(i) as usize];
                    if continental(i) {
                        c
                    } else {
                        lerp_color(c, hex(SURFACE), 0.55)
                    }
                })
                .collect()
        }
        "crust_type" => (0..n)
            .map(|i| {
                hex(if continental(i) {
                    CONTINENT_FILL
                } else {
                    OCEAN_FILL
                })
            })
            .collect(),
        "crust_age" => (0..n)
            .map(|i| {
                if continental(i) {
                    hex(CONTINENT_GREY)
                } else {
                    blues(v(i) / 200.0)
                }
            })
            .collect(),
        "crust_thickness" | "moho_depth" => (0..n).map(|i| blues(v(i) / 6_000.0)).collect(),
        "history_flags" => (0..n)
            .map(|i| {
                let f = v(i) as u8;
                FLAG_ORDER
                    .iter()
                    .find(|(bit, _, _)| f & bit != 0)
                    .map(|(_, c, _)| hex(c))
                    .unwrap_or_else(|| faded_elevation(elev(i)))
            })
            .collect(),
        "hotspot_track" => (0..n)
            .map(|i| {
                if v(i) >= 0.0 {
                    lerp_color(hex(SERIES[3]), hex(SERIES[6]), v(i) / HOTSPOT_AGE_SCALE)
                } else {
                    faded_elevation(elev(i))
                }
            })
            .collect(),
        _ => return None,
    };
    Some(colors)
}

/// Legend entries of a layer.
pub fn legend(name: &str) -> Vec<(Color, &'static str)> {
    match name {
        "elevation_tectonic" => vec![
            (elevation_color(-3_200.0), "-3200 m"),
            (elevation_color(-1_500.0), "-1500"),
            (elevation_color(-100.0), "-100"),
            (elevation_color(200.0), "200"),
            (elevation_color(1_000.0), "1000"),
            (elevation_color(2_000.0), "2000"),
            (elevation_color(3_000.0), "3000"),
        ],
        "plate_id" => vec![
            (hex(SERIES[0]), "continental crust (saturated)"),
            (
                lerp_color(hex(SERIES[0]), hex(SURFACE), 0.55),
                "oceanic (pale)",
            ),
            (hex(INK), "boundary, velocity"),
        ],
        "crust_type" => vec![
            (hex(OCEAN_FILL), "oceanic"),
            (hex(CONTINENT_FILL), "continental"),
        ],
        "crust_age" => vec![
            (blues(0.0), "0 Ma"),
            (blues(0.25), "50"),
            (blues(0.5), "100"),
            (blues(1.0), "200+"),
            (hex(CONTINENT_GREY), "continent"),
        ],
        "crust_thickness" => vec![
            (blues(700.0 / 6_000.0), "700 m"),
            (blues(3_500.0 / 6_000.0), "3500"),
            (blues(1.0), "6000"),
        ],
        "moho_depth" => vec![(blues(0.5), "3000 m below sea level"), (blues(1.0), "6000")],
        "history_flags" => FLAG_ORDER.iter().map(|(_, c, l)| (hex(c), *l)).collect(),
        "hotspot_track" => vec![
            (hex(SERIES[3]), "0 Ma"),
            (lerp_color(hex(SERIES[3]), hex(SERIES[6]), 0.5), "60"),
            (hex(SERIES[6]), "120+ Ma"),
        ],
        _ => Vec::new(),
    }
}

/// Upscale factor so a map is at least ~500 px wide.
fn factor(res: usize) -> u32 {
    (500usize.div_ceil(res)).max(1) as u32
}

fn upscale(colors: &[Color], w: usize, h: usize) -> RgbImage {
    let k = factor(w);
    let mut img = RgbImage::new(w as u32 * k, h as u32 * k);
    for (idx, &c) in colors.iter().enumerate() {
        let (i, j) = ((idx % w) as u32, (idx / w) as u32);
        for y in 0..k {
            for x in 0..k {
                img.put_pixel(i * k + x, j * k + y, c);
            }
        }
    }
    img
}

/// Torus-aware mean position of each plate's cells (circular mean on both axes).
fn plate_centres(plate_id: &PeriodicGrid<u8>, n: usize) -> Vec<(f64, f64)> {
    let mut acc = vec![[0f64; 4]; n];
    for idx in 0..plate_id.data().len() {
        let (i, j) = plate_id.coords(idx);
        let (x, z) = plate_id.cell_center(i, j);
        let (ax, az) = (TAU * x / WORLD_SIZE_F64, TAU * z / WORLD_SIZE_F64);
        let a = &mut acc[plate_id.data()[idx] as usize];
        a[0] += ax.cos();
        a[1] += ax.sin();
        a[2] += az.cos();
        a[3] += az.sin();
    }
    acc.iter()
        .map(|a| {
            let x = a[1].atan2(a[0]).rem_euclid(TAU) / TAU * WORLD_SIZE_F64;
            let z = a[3].atan2(a[2]).rem_euclid(TAU) / TAU * WORLD_SIZE_F64;
            (x, z)
        })
        .collect()
}

/// Darkens pixels on plate boundaries.
fn draw_boundaries(cv: &mut Canvas, plate_id: &PeriodicGrid<u8>) {
    let k = factor(plate_id.width()) as i32;
    let ink = hex(INK);
    for idx in 0..plate_id.data().len() {
        let (i, j) = plate_id.coords(idx);
        let a = plate_id.data()[idx];
        if *plate_id.get(i as i64 + 1, j as i64) != a {
            cv.vline(
                (i as i32 + 1) * k - 1,
                j as i32 * k,
                (j as i32 + 1) * k - 1,
                1,
                ink,
            );
        }
        if *plate_id.get(i as i64, j as i64 + 1) != a {
            cv.hline(
                i as i32 * k,
                (i as i32 + 1) * k - 1,
                (j as i32 + 1) * k - 1,
                1,
                ink,
            );
        }
    }
}

/// Velocity arrows (length ∝ speed, 60 px for the fastest plate) and plate ids.
fn draw_velocities(cv: &mut Canvas, plate_id: &PeriodicGrid<u8>, plates: &[Plate]) {
    let px = f64::from(cv.img.width()) / WORLD_SIZE_F64;
    let max_v = plates
        .iter()
        .map(|p| p.velocity.0.hypot(p.velocity.1))
        .fold(1e-9, f64::max);
    let ink = hex(INK);
    for (p, (cx, cz)) in plates.iter().zip(plate_centres(plate_id, plates.len())) {
        let speed = p.velocity.0.hypot(p.velocity.1);
        let len = 60.0 * speed / max_v;
        let (x0, y0) = (cx * px, cz * px);
        cv.rect(x0 as i32 - 3, y0 as i32 - 3, 7, 7, ink);
        if len > 1.0 {
            let (ux, uy) = (p.velocity.0 / speed, p.velocity.1 / speed);
            let (x1, y1) = (x0 + ux * len, y0 + uy * len);
            cv.polyline(&[(x0, y0), (x1, y1)], 3, ink, None);
            let head = |s: f64| (x1 - 12.0 * ux + s * 7.0 * uy, y1 - 12.0 * uy - s * 7.0 * ux);
            cv.polyline(&[head(1.0), (x1, y1), head(-1.0)], 3, ink, None);
        }
        cv.text(x0 as i32 + 6, y0 as i32 + 6, &format!("{}", p.id), 2, ink);
    }
}

/// Adds a legend strip under a map.
fn with_legend(img: RgbImage, items: &[(Color, &str)]) -> Canvas {
    let line = text_height(2) + 10;
    let item_w = |label: &str| 26 + text_width(label, 2) + 20;
    let mut rows = 1;
    let mut x = 10;
    for (_, label) in items {
        if x + item_w(label) > img.width() as i32 {
            rows += 1;
            x = 10;
        }
        x += item_w(label);
    }
    let mut cv = Canvas::new(
        img.width(),
        img.height() + (rows * line + 10) as u32,
        hex(SURFACE),
    );
    image::imageops::replace(&mut cv.img, &img, 0, 0);
    let (mut x, mut y) = (10, img.height() as i32 + 10);
    for (c, label) in items {
        if x + item_w(label) > img.width() as i32 {
            x = 10;
            y += line;
        }
        cv.rect(x, y + 2, 18, 10, *c);
        cv.text(x + 26, y, label, 2, hex(INK_2));
        x += item_w(label);
    }
    cv
}

/// Writes one preview per layer, plus `elevation_tiled.png` (2×2 seam check), into `dir`.
/// Plate velocities, when given, are drawn on the plate map.
pub fn write_previews(world: &World, plates: &[Plate], dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let mut written = Vec::new();
    let plate_grid = world.layer("plate_id").and_then(u8_grid);
    for layer in &world.layers {
        let Some(colors) = colorize(world, &layer.name) else {
            continue;
        };
        let img = upscale(&colors, layer.width, layer.height);
        if layer.name == "elevation_tectonic" {
            let mut tiled = RgbImage::new(img.width() * 2, img.height() * 2);
            for (ox, oy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                image::imageops::replace(
                    &mut tiled,
                    &img,
                    i64::from(ox * img.width()),
                    i64::from(oy * img.height()),
                );
            }
            let p = dir.join("elevation_tiled.png");
            Canvas { img: tiled }.save(&p)?;
            written.push(p);
        }
        let mut cv = Canvas { img };
        if let Some(pg) = &plate_grid {
            match layer.name.as_str() {
                "plate_id" => {
                    draw_boundaries(&mut cv, pg);
                    draw_velocities(&mut cv, pg, plates);
                }
                "history_flags" => draw_boundaries(&mut cv, pg),
                _ => {}
            }
        }
        let cv = with_legend(cv.img, &legend(&layer.name));
        let p = dir.join(format!("{}.png", layer.name));
        cv.save(&p)?;
        written.push(p);
    }
    Ok(written)
}
