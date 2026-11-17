//! Vertical cross-sections of the subsurface (spec §7): rock column, ore bodies, Moho and
//! isotherms along a line on the map. Used by the viewer and by the `section` tool.

use nascio_core::GeothermConfig;
use nascio_core::fmath::DetMath;
use nascio_core::torus;
use nascio_world::deposits::Catalog;
use nascio_world::grid::PeriodicGrid;
use nascio_world::world_io::World;
use nascio_worldgen::plot::{Canvas, rgb, text_width};

use nascio_voxel::Terrain;
use nascio_voxel::block::BlockId;

/// Isotherms drawn on sections, °C.
pub const ISOTHERMS_C: [f64; 5] = [50.0, 100.0, 300.0, 600.0, 900.0];

/// What a section shows.
#[derive(Clone, Debug)]
pub struct SectionSpec {
    /// Ends of the line (world m; the shorter way round the torus).
    pub from: (f64, f64),
    pub to: (f64, f64),
    /// Heights of the top and bottom of the picture, m.
    pub top_m: f64,
    pub bottom_m: f64,
    /// Size of the drawing area, px (margins and legend come on top).
    pub width: u32,
    pub height: u32,
    pub title: String,
}

/// One column of a section, for the HUD or the legend.
#[derive(Clone, Debug)]
pub struct SectionColumn {
    pub x: f64,
    pub z: f64,
    pub top: i32,
    pub water_top: i32,
    pub moho: i32,
    /// Heights of the isotherms [`ISOTHERMS_C`].
    pub isotherms: [f64; ISOTHERMS_C.len()],
}

/// Surface temperature of the rock (mean annual air on land, sea floor water).
pub struct SurfaceTemperature {
    mean_air: Option<PeriodicGrid<f32>>,
    geotherm: GeothermConfig,
}

impl SurfaceTemperature {
    pub fn from_world(world: &World) -> Self {
        let mean_air = world.layer("temperature_x0").map(|l| {
            let data = (0..l.width * l.height)
                .map(|k| l.data.get_f64(k) as f32)
                .collect();
            PeriodicGrid::from_vec(l.width, l.height, l.cell_m, data)
        });
        Self {
            mean_air,
            geotherm: world.config.geotherm,
        }
    }

    pub fn geotherm(&self) -> &GeothermConfig {
        &self.geotherm
    }

    /// Rock surface temperature at `(x, z)` under `water` m of water, °C.
    pub fn at(&self, x: f64, z: f64, water: f64) -> f64 {
        let air = self
            .mean_air
            .as_ref()
            .map_or(10.0, |g| g.sample_bilinear(x, z));
        self.geotherm.surface_c(air, water)
    }

    /// Rock temperature `depth` m below the rock surface of a column whose top and Moho are
    /// `top` and `moho`.
    pub fn rock(&self, x: f64, z: f64, top: f64, water_top: f64, moho: f64, depth: f64) -> f64 {
        let ts = self.at(x, z, (water_top - top).max(0.0));
        if depth >= top - moho {
            return self.geotherm.moho_c;
        }
        self.geotherm.temperature(ts, depth, (top - moho).max(1.0))
    }
}

/// Columns of a section, `n` samples along the line.
pub fn columns(
    terrain: &Terrain,
    temp: &SurfaceTemperature,
    spec: &SectionSpec,
    n: usize,
) -> Vec<SectionColumn> {
    let d = torus::delta2_f64(spec.from, spec.to);
    (0..n)
        .map(|k| {
            let t = (k as f64 + 0.5) / n as f64;
            let x = torus::wrap_f64(spec.from.0 + d.0 * t);
            let z = torus::wrap_f64(spec.from.1 + d.1 * t);
            let top = terrain.height(x, z).floor() as i32;
            let water_top = terrain.water_top(x, z);
            let moho = terrain.moho_y(x, z, top);
            let crust = f64::from((top - moho).max(1));
            let ts = temp.at(x, z, f64::from((water_top - top).max(0)));
            let isotherms =
                ISOTHERMS_C.map(|c| f64::from(top) - temp.geotherm.depth_of(ts, crust, c));
            SectionColumn {
                x,
                z,
                top,
                water_top,
                moho,
                isotherms,
            }
        })
        .collect()
}

/// Renders a section with axes and legend.
pub fn render(
    terrain: &Terrain,
    temp: &SurfaceTemperature,
    catalog: Option<&Catalog>,
    spec: &SectionSpec,
) -> Canvas {
    let (ml, mr, mt, mb) = (78i32, 250i32, 34i32, 46i32);
    let (w, h) = (spec.width as i32, spec.height as i32);
    let bg = rgb([246, 246, 242]);
    let mut cv = Canvas::new((w + ml + mr) as u32, (h + mt + mb) as u32, bg);
    let ink = rgb([30, 30, 34]);
    let blocks = terrain.blocks();
    let cols = columns(terrain, temp, spec, w as usize);
    let span = spec.top_m - spec.bottom_m;
    let py_of = |y: f64| mt as f64 + (spec.top_m - y) / span * h as f64;
    let mut used = vec![false; blocks.len()];
    let mut ores_used = vec![false; catalog.map_or(0, |c| c.deposits.len())];
    let water = blocks.def(blocks.ids.water).color;
    let magma = blocks.def(blocks.ids.magma).color;
    for (px, c) in cols.iter().enumerate() {
        let geo = terrain.column_geo(c.x, c.z);
        let (bx, bz) = (c.x.floor() as i32, c.z.floor() as i32);
        let slope = {
            let e = 4.0;
            let gx = terrain.height(c.x + e, c.z) - terrain.height(c.x - e, c.z);
            let gz = terrain.height(c.x, c.z + e) - terrain.height(c.x, c.z - e);
            gx.det_hypot(gz) / (2.0 * e)
        };
        let bodies = terrain.geology().bodies_near(c.x - 1.0, c.z - 1.0, 2.0);
        for py in 0..h {
            let y = spec.top_m - (py as f64 + 0.5) / h as f64 * span;
            let yi = y.floor() as i32;
            let color = if yi > c.top {
                if yi < c.water_top {
                    water
                } else {
                    [214, 230, 244]
                }
            } else if yi < c.moho {
                used[usize::from(blocks.ids.magma)] = true;
                magma
            } else {
                let (b, class): (BlockId, _) =
                    terrain.rock_block(&geo, bx, bz, c.top, slope, c.moho, c.top - yi);
                let ore = terrain
                    .geology()
                    .ore_kind(&bodies, class, bx, yi, bz, c.top);
                match (ore, catalog) {
                    (Some(k), Some(cat)) if usize::from(k) < cat.deposits.len() => {
                        ores_used[usize::from(k)] = true;
                        cat.deposits[usize::from(k)].map_color
                    }
                    _ => {
                        used[usize::from(b)] = true;
                        blocks.def(b).color
                    }
                }
            };
            cv.put(ml + px as i32, mt + py, rgb(color));
        }
    }
    // Moho, isotherms, surface.
    let line = |f: &dyn Fn(&SectionColumn) -> f64| -> Vec<(f64, f64)> {
        cols.iter()
            .enumerate()
            .map(|(px, c)| {
                (
                    (ml + px as i32) as f64,
                    py_of(f(c)).clamp(mt as f64, (mt + h) as f64),
                )
            })
            .collect()
    };
    let iso_colors = [
        [250, 250, 160],
        [255, 220, 120],
        [255, 160, 70],
        [235, 90, 40],
        [190, 20, 20],
    ];
    for (k, color) in iso_colors.iter().enumerate() {
        cv.polyline(&line(&|c| c.isotherms[k]), 1, rgb(*color), Some((6.0, 4.0)));
    }
    cv.polyline(&line(&|c| f64::from(c.moho)), 2, rgb([200, 0, 60]), None);
    cv.polyline(&line(&|c| f64::from(c.top) + 1.0), 1, ink, None);

    // Frame and axes.
    cv.hline(ml, ml + w, mt + h, 1, ink);
    cv.vline(ml - 1, mt, mt + h, 1, ink);
    let step = if span > 6_000.0 { 1_000.0 } else { 500.0 };
    let mut y = (spec.bottom_m / step).ceil() * step;
    while y <= spec.top_m {
        let py = py_of(y) as i32;
        cv.hline(ml - 6, ml - 1, py, 1, ink);
        let s = format!("{y:.0}");
        cv.text(ml - 10 - text_width(&s, 1), py - 3, &s, 1, ink);
        y += step;
    }
    cv.text(4, mt - 14, "height (m)", 1, ink);
    let d = torus::delta2_f64(spec.from, spec.to);
    let len = d.0.det_hypot(d.1);
    let dstep = if len > 20_000.0 {
        5_000.0
    } else if len > 6_000.0 {
        2_000.0
    } else {
        500.0
    };
    let mut s = 0.0;
    while s <= len + 1e-6 {
        let px = ml + (s / len * w as f64) as i32;
        cv.vline(px, mt + h, mt + h + 5, 1, ink);
        let label = if dstep < 1_000.0 {
            format!("{:.1}", s / 1_000.0)
        } else {
            format!("{:.0}", s / 1_000.0)
        };
        cv.text(px - text_width(&label, 1) / 2, mt + h + 9, &label, 1, ink);
        s += dstep;
    }
    let caption = format!(
        "distance (km) from ({:.0}, {:.0}) to ({:.0}, {:.0}) - vertical exaggeration x{:.1}",
        spec.from.0,
        spec.from.1,
        spec.to.0,
        spec.to.1,
        (len / w as f64) / (span / h as f64)
    );
    cv.text(ml, mt + h + 26, &caption, 1, ink);
    cv.text(ml, 10, &spec.title, 2, ink);

    // Legend.
    let lx = ml + w + 16;
    let mut ly = mt;
    let entry = |cv: &mut Canvas, ly: &mut i32, color: [u8; 3], label: &str| {
        cv.rect(lx, *ly, 14, 10, rgb(color));
        cv.text(lx + 20, *ly + 2, label, 1, ink);
        *ly += 15;
    };
    for (i, u) in used.iter().enumerate() {
        if *u {
            let def = blocks.def(i as BlockId);
            entry(&mut cv, &mut ly, def.color, &def.name);
        }
    }
    if let Some(cat) = catalog {
        for (k, u) in ores_used.iter().enumerate() {
            if *u {
                let d = &cat.deposits[k];
                entry(
                    &mut cv,
                    &mut ly,
                    d.map_color,
                    &format!("ore: {}", d.label.to_lowercase()),
                );
            }
        }
    }
    ly += 6;
    entry(&mut cv, &mut ly, [200, 0, 60], "Moho");
    for (k, c) in ISOTHERMS_C.iter().enumerate() {
        entry(
            &mut cv,
            &mut ly,
            iso_colors[k],
            &format!("{c:.0} C isotherm"),
        );
    }
    cv
}
