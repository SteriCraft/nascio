//! PNG previews of the tectonic layers (`previews/*.png`).

use std::f64::consts::TAU;
use std::path::{Path, PathBuf};

use image::{Rgb, RgbImage};
use nascio_core::consts::WORLD_SIZE_F64;

use crate::grid::PeriodicGrid;
use crate::plot::{Canvas, Color, hex, text_height, text_width};
use crate::tectonics::{CONTINENTAL, TectonicOutput, flags};

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

/// Upscale factor so a map is at least ~500 px wide.
fn factor(res: usize) -> u32 {
    (500usize.div_ceil(res)).max(1) as u32
}

fn image_from<T>(g: &PeriodicGrid<T>, mut f: impl FnMut(usize) -> Color) -> RgbImage {
    let k = factor(g.width());
    let mut img = RgbImage::new(g.width() as u32 * k, g.height() as u32 * k);
    for (idx, _) in g.data().iter().enumerate() {
        let (i, j) = g.coords(idx);
        let c = f(idx);
        for y in 0..k {
            for x in 0..k {
                img.put_pixel(i as u32 * k + x, j as u32 * k + y, c);
            }
        }
    }
    img
}

/// Greedy colouring of the plate adjacency graph with the 8 categorical colours, so
/// neighbouring plates never share a colour (falls back to cycling if impossible).
fn plate_colors(plate_id: &PeriodicGrid<u8>, n: usize) -> Vec<Color> {
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

/// Adds a legend strip under a map: `(colour, label)` pairs.
fn with_legend(img: RgbImage, items: &[(Color, &str)]) -> Canvas {
    let line = text_height(2) + 10;
    let mut rows = 1;
    let mut x = 10;
    for (_, label) in items {
        let wdt = 26 + text_width(label, 2) + 20;
        if x + wdt > img.width() as i32 {
            rows += 1;
            x = 10;
        }
        x += wdt;
    }
    let mut cv = Canvas::new(
        img.width(),
        img.height() + (rows * line + 10) as u32,
        hex(SURFACE),
    );
    image::imageops::replace(&mut cv.img, &img, 0, 0);
    let (mut x, mut y) = (10, img.height() as i32 + 10);
    for (c, label) in items {
        let wdt = 26 + text_width(label, 2) + 20;
        if x + wdt > img.width() as i32 {
            x = 10;
            y += line;
        }
        cv.rect(x, y + 2, 18, 10, *c);
        cv.text(x + 26, y, label, 2, hex(INK_2));
        x += wdt;
    }
    cv
}

/// Writes every preview into `dir` and returns the paths.
pub fn write_previews(out: &TectonicOutput, dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let mut written = Vec::new();
    let mut save = |name: &str, cv: Canvas| -> anyhow::Result<()> {
        let p = dir.join(format!("{name}.png"));
        cv.save(&p)?;
        written.push(p);
        Ok(())
    };
    let elev = out.elevation.data();
    let crust = out.crust_type.data();

    // Elevation.
    let img = image_from(&out.elevation, |i| elevation_color(f64::from(elev[i])));
    save(
        "elevation_tectonic",
        with_legend(
            img.clone(),
            &[
                (elevation_color(-3_200.0), "-3200 m"),
                (elevation_color(-1_500.0), "-1500"),
                (elevation_color(-100.0), "-100"),
                (elevation_color(200.0), "200"),
                (elevation_color(1_000.0), "1000"),
                (elevation_color(2_000.0), "2000"),
                (elevation_color(3_000.0), "3000"),
            ],
        ),
    )?;

    // Seam check: the elevation map tiled 2×2.
    let mut tiled = RgbImage::new(img.width() * 2, img.height() * 2);
    for (ox, oy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        image::imageops::replace(
            &mut tiled,
            &img,
            i64::from(ox * img.width()),
            i64::from(oy * img.height()),
        );
    }
    save("elevation_tiled", Canvas { img: tiled })?;

    // Plates, boundaries and velocity arrows.
    let n = out.plates.len();
    let colors = plate_colors(&out.plate_id, n);
    let pid = out.plate_id.data();
    let mut cv = Canvas {
        img: image_from(&out.plate_id, |i| {
            let c = colors[pid[i] as usize];
            if crust[i] == CONTINENTAL {
                c
            } else {
                lerp_color(c, hex(SURFACE), 0.55)
            }
        }),
    };
    draw_boundaries(&mut cv, &out.plate_id);
    let px = cv.img.width() as f64 / WORLD_SIZE_F64;
    let max_v = out
        .plates
        .iter()
        .map(|p| p.velocity.0.hypot(p.velocity.1))
        .fold(1e-9, f64::max);
    for (p, (cx, cz)) in out.plates.iter().zip(plate_centres(&out.plate_id, n)) {
        // Arrow length ∝ speed (60 px for the fastest plate).
        let speed = p.velocity.0.hypot(p.velocity.1);
        let len = 60.0 * speed / max_v;
        let (x0, y0) = (cx * px, cz * px);
        let ink = hex(INK);
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
    save(
        "plate_id",
        with_legend(
            cv.img,
            &[
                (hex(SERIES[0]), "continental crust (saturated)"),
                (
                    lerp_color(hex(SERIES[0]), hex(SURFACE), 0.55),
                    "oceanic (pale)",
                ),
                (hex(INK), "boundary, velocity"),
            ],
        ),
    )?;

    // Crust type.
    let (oc, co) = (hex("#2f6db0"), hex("#c9a66b"));
    let img = image_from(&out.crust_type, |i| {
        if crust[i] == CONTINENTAL { co } else { oc }
    });
    save(
        "crust_type",
        with_legend(img, &[(oc, "oceanic"), (co, "continental")]),
    )?;

    // Crust age (oceanic only).
    let age = out.crust_age.data();
    let img = image_from(&out.crust_age, |i| {
        if crust[i] == CONTINENTAL {
            hex(CONTINENT_GREY)
        } else {
            blues(f64::from(age[i]) / 200.0)
        }
    });
    save(
        "crust_age",
        with_legend(
            img,
            &[
                (blues(0.0), "0 Ma"),
                (blues(0.25), "50"),
                (blues(0.5), "100"),
                (blues(1.0), "200+"),
                (hex(CONTINENT_GREY), "continent"),
            ],
        ),
    )?;

    // Thickness and Moho depth.
    let thick = out.crust_thickness.data();
    let img = image_from(&out.crust_thickness, |i| {
        blues(f64::from(thick[i]) / 6_000.0)
    });
    save(
        "crust_thickness",
        with_legend(
            img,
            &[
                (blues(700.0 / 6000.0), "700 m"),
                (blues(3500.0 / 6000.0), "3500"),
                (blues(1.0), "6000"),
            ],
        ),
    )?;
    let moho = out.moho_depth.data();
    let img = image_from(&out.moho_depth, |i| blues(f64::from(moho[i]) / 6_000.0));
    save(
        "moho_depth",
        with_legend(
            img,
            &[(blues(0.5), "3000 m below sea level"), (blues(1.0), "6000")],
        ),
    )?;

    // History flags over a faded relief; the first matching flag in this order wins.
    let order: [(u8, &str, &str); 7] = [
        (flags::HOTSPOT, SERIES[3], "hotspot"),
        (flags::ARC, SERIES[7], "arc"),
        (flags::TRENCH, SERIES[6], "trench"),
        (flags::OROGENY, SERIES[1], "orogeny"),
        (flags::OPHIOLITE, SERIES[4], "ophiolite"),
        (flags::RIFT, SERIES[2], "rift"),
        (flags::RIDGE, SERIES[0], "ridge"),
    ];
    let fl = out.history_flags.data();
    let img = image_from(&out.history_flags, |i| {
        order
            .iter()
            .find(|(bit, _, _)| fl[i] & bit != 0)
            .map(|(_, c, _)| hex(c))
            .unwrap_or_else(|| faded_elevation(f64::from(elev[i])))
    });
    let legend: Vec<(Color, &str)> = order.iter().map(|(_, c, l)| (hex(c), *l)).collect();
    let mut cv = with_legend(img, &legend);
    draw_boundaries(&mut cv, &out.plate_id);
    save("history_flags", cv)?;

    // Hotspot tracks: young (yellow) to old (violet) over a faded relief.
    let track = out.hotspot_track.data();
    let (young, old) = (hex(SERIES[3]), hex(SERIES[6]));
    let img = image_from(&out.hotspot_track, |i| {
        if track[i] >= 0.0 {
            lerp_color(young, old, f64::from(track[i]) / 60.0)
        } else {
            faded_elevation(f64::from(elev[i]))
        }
    });
    save(
        "hotspot_track",
        with_legend(
            img,
            &[
                (young, "0 Ma"),
                (lerp_color(young, old, 0.5), "30"),
                (old, "60+ Ma"),
            ],
        ),
    )?;

    Ok(written)
}
