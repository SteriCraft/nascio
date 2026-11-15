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
    let p = dir.join("hypsometry.png");
    write_hypsometry(world, &p)?;
    written.push(p);
    Ok(written)
}

/// Bin width of the hypsometry histogram, m.
const HYPSO_BIN_M: f64 = 100.0;
/// Range of the hypsometry histogram, m.
const HYPSO_RANGE: (f64, f64) = (-3_600.0, 3_100.0);

/// Target share (%) of the surface in `[lo, hi)`, from piecewise-uniform bands.
fn band_share(bands: &[[f64; 3]], lo: f64, hi: f64, total: f64) -> f64 {
    bands
        .iter()
        .map(|b| {
            let overlap = (hi.min(b[1]) - lo.max(b[0])).max(0.0);
            b[2] * overlap / (b[1] - b[0])
        })
        .sum::<f64>()
        * 100.0
        / total
}

/// Histogram of elevations (bars, % of the surface per 100 m) against the target hypsometry
/// (step line): Earth's curve scaled to the spec §3 targets.
pub fn write_hypsometry(world: &World, path: &Path) -> anyhow::Result<()> {
    let Some(elev) = world.layer("elevation_tectonic") else {
        return Ok(());
    };
    let h = &world.config.tectonics.hypsometry;
    let t = &world.config.tectonics;
    let shelf = [[t.shelf_depth_m, 0.0, h.shelf_pct]];
    let slope = [[-1_500.0, t.shelf_depth_m, h.slope_pct]];
    let total: f64 =
        h.land.iter().chain(&h.deep).map(|b| b[2]).sum::<f64>() + h.shelf_pct + h.slope_pct;
    let n_bins = ((HYPSO_RANGE.1 - HYPSO_RANGE.0) / HYPSO_BIN_M) as usize;
    let n = elev.width * elev.height;
    let mut counts = vec![0usize; n_bins];
    for i in 0..n {
        let b = ((elev.data.get_f64(i) - HYPSO_RANGE.0) / HYPSO_BIN_M).floor();
        counts[(b.max(0.0) as usize).min(n_bins - 1)] += 1;
    }
    let world_pct: Vec<f64> = counts
        .iter()
        .map(|&c| 100.0 * c as f64 / n as f64)
        .collect();
    let target_pct: Vec<f64> = (0..n_bins)
        .map(|k| {
            let lo = HYPSO_RANGE.0 + k as f64 * HYPSO_BIN_M;
            let hi = lo + HYPSO_BIN_M;
            [&h.land[..], &h.deep[..], &shelf[..], &slope[..]]
                .iter()
                .map(|bands| band_share(bands, lo, hi, total))
                .sum()
        })
        .collect();
    let y_max = world_pct
        .iter()
        .chain(&target_pct)
        .fold(1.0f64, |m, &v| m.max(v))
        .ceil();

    let (w, hgt) = (1_100, 540);
    let (left, right, top, bottom) = (70, 30, 110, 50);
    let mut cv = Canvas::new(w as u32, hgt as u32, hex(SURFACE));
    let (ink, ink2, grid) = (hex(INK), hex(INK_2), hex("#e4e3df"));
    let (bar_c, line_c) = (hex(SERIES[0]), hex(SERIES[1]));
    let px = |v: f64| {
        left as f64
            + (v - HYPSO_RANGE.0) / (HYPSO_RANGE.1 - HYPSO_RANGE.0) * (w - left - right) as f64
    };
    let py = |p: f64| (hgt - bottom) as f64 - p / y_max * (hgt - top - bottom) as f64;

    let land = (0..n).filter(|&i| elev.data.get_f64(i) > 0.0).count() as f64 / n as f64;
    let crust = world.layer("crust_type").map_or(0.0, |l| {
        (0..n)
            .filter(|&i| l.data.get_f64(i) as u8 == CONTINENTAL)
            .count() as f64
            / n as f64
    });
    cv.text(
        left,
        14,
        "hypsometry: share of the surface per 100 m of elevation",
        2,
        ink,
    );
    cv.text(
        left,
        38,
        &format!(
            "seed {}, scale 1/{}: land {:.1}%, continental crust {:.1}%",
            world.config.seed,
            world.config.scale.divisor(),
            100.0 * land,
            100.0 * crust
        ),
        2,
        ink2,
    );
    cv.rect(left, 72, 18, 10, bar_c);
    cv.text(left + 26, 68, "this world", 2, ink2);
    cv.rect(left + 200, 75, 18, 3, line_c);
    cv.text(
        left + 226,
        68,
        "target: earth hypsometry scaled to spec 3",
        2,
        ink2,
    );

    // Recessive grid and axes.
    let step = if y_max > 8.0 { 2.0 } else { 1.0 };
    let mut p = 0.0;
    while p <= y_max + 1e-9 {
        let y = py(p).round() as i32;
        cv.hline(left, w - right, y, 1, grid);
        let label = format!("{p:.0}%");
        cv.text(
            left - 8 - text_width(&label, 2),
            y - text_height(2) / 2,
            &label,
            2,
            ink2,
        );
        p += step;
    }
    let mut e = -3_500.0;
    while e <= 3_000.0 {
        let x = px(e).round() as i32;
        let label = format!("{e:.0}");
        cv.text(
            x - text_width(&label, 2) / 2,
            hgt - bottom + 10,
            &label,
            2,
            ink2,
        );
        e += 500.0;
    }
    // Bars with a 2 px gap.
    for (k, &v) in world_pct.iter().enumerate() {
        let lo = HYPSO_RANGE.0 + k as f64 * HYPSO_BIN_M;
        let (x0, x1) = (
            px(lo).round() as i32 + 1,
            px(lo + HYPSO_BIN_M).round() as i32 - 1,
        );
        let y = py(v).round() as i32;
        cv.rect(x0, y, (x1 - x0).max(1), hgt - bottom - y, bar_c);
    }
    // Sea level.
    cv.vline(px(0.0).round() as i32, top, hgt - bottom, 1, ink2);
    cv.text(px(0.0) as i32 + 4, top, "sea level", 2, ink2);
    // Target step line.
    let mut pts = Vec::new();
    for (k, &v) in target_pct.iter().enumerate() {
        let lo = HYPSO_RANGE.0 + k as f64 * HYPSO_BIN_M;
        pts.push((px(lo), py(v)));
        pts.push((px(lo + HYPSO_BIN_M), py(v)));
    }
    cv.polyline(&pts, 2, line_c, None);
    cv.hline(left, w - right, hgt - bottom, 1, ink);
    cv.save(path)
}
