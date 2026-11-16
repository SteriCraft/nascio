//! Geology (spec §4.6): geological contexts, sedimentary cover and weathering, on the tectonic
//! grid (plus the placer sources along the rivers of the fine grid).
//!
//! Every cell gets a weight in `[0, 1]` for each [`Context`]. They are derived from the crust
//! (type, age), the tectonic history flags, the sedimentary cover, the climate and the rivers.
//! The deposit catalogue ([`crate::deposits`]) only refers to these contexts by name.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::climate::ClimateOutput;
use crate::config::WorldConfig;
use crate::erosion::ErosionOutput;
use crate::erosion::flow::{D8, NO_DIR};
use crate::grid::PeriodicGrid;
use crate::noise::Fbm;
use crate::rng::stage_u64;
use crate::tectonics::{CONTINENTAL, TectonicOutput, flags};
use crate::world_io::Layer;

/// Geological contexts known to the code (names used by the deposit catalogue).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Context {
    Ocean,
    Ridge,
    Trench,
    Arc,
    Orogen,
    Rift,
    Hotspot,
    Ophiolite,
    Ultramafic,
    Craton,
    Platform,
    Basin,
    MarineShelf,
    Humid,
    Arid,
    Laterite,
    RiverSource,
    Coast,
}

/// Number of contexts.
pub const CONTEXTS: usize = 18;

impl Context {
    pub const ALL: [Context; CONTEXTS] = [
        Context::Ocean,
        Context::Ridge,
        Context::Trench,
        Context::Arc,
        Context::Orogen,
        Context::Rift,
        Context::Hotspot,
        Context::Ophiolite,
        Context::Ultramafic,
        Context::Craton,
        Context::Platform,
        Context::Basin,
        Context::MarineShelf,
        Context::Humid,
        Context::Arid,
        Context::Laterite,
        Context::RiverSource,
        Context::Coast,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Context::Ocean => "ocean",
            Context::Ridge => "ridge",
            Context::Trench => "trench",
            Context::Arc => "arc",
            Context::Orogen => "orogen",
            Context::Rift => "rift",
            Context::Hotspot => "hotspot",
            Context::Ophiolite => "ophiolite",
            Context::Ultramafic => "ultramafic",
            Context::Craton => "craton",
            Context::Platform => "platform",
            Context::Basin => "basin",
            Context::MarineShelf => "marine_shelf",
            Context::Humid => "humid",
            Context::Arid => "arid",
            Context::Laterite => "laterite",
            Context::RiverSource => "river_source",
            Context::Coast => "coast",
        }
    }

    pub fn from_name(name: &str) -> Option<Context> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Dominant geological setting of a cell (`geo_context` layer): what the rock column is built
/// from. Codes are stable within a generator version.
pub mod setting {
    pub const OCEAN: u8 = 0;
    pub const RIDGE: u8 = 1;
    pub const TRENCH: u8 = 2;
    pub const ARC: u8 = 3;
    pub const OROGEN: u8 = 4;
    pub const RIFT: u8 = 5;
    pub const HOTSPOT: u8 = 6;
    pub const OPHIOLITE: u8 = 7;
    pub const CRATON: u8 = 8;
    pub const PLATFORM: u8 = 9;
    pub const BASIN: u8 = 10;
    pub const MARINE_SHELF: u8 = 11;
    pub const COUNT: usize = 12;

    pub const NAMES: [&str; COUNT] = [
        "ocean",
        "ridge",
        "trench",
        "volcanic arc",
        "orogen",
        "rift",
        "hotspot",
        "ophiolite",
        "craton",
        "platform",
        "sedimentary basin",
        "marine shelf",
    ];

    /// Whether the basement of this setting is oceanic crust.
    pub fn oceanic(code: u8) -> bool {
        matches!(code, OCEAN | RIDGE | TRENCH)
    }
}

/// Tunable parameters of the geology (`[geology]` in TOML).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeologyConfig {
    /// Thickest sedimentary cover, m.
    pub sediment_max_m: f64,
    /// Continental lowlands below this (smoothed tectonic) elevation fill with sediments, m.
    pub basin_high_m: f64,
    /// Share of `sediment_max_m` in the deepest continental basins.
    pub basin_fill: f64,
    /// Share of `sediment_max_m` on continental shelves and passive margins.
    pub shelf_fill: f64,
    /// Share of `sediment_max_m` in rift grabens.
    pub rift_fill: f64,
    /// Cratons keep this share of the basin cover (thin platform cover).
    pub craton_cover: f64,
    /// Pelagic sediment accumulation on the ocean floor, m per Ma of crust age.
    pub pelagic_m_per_ma: f64,
    /// Thickest pelagic cover, m.
    pub pelagic_max_m: f64,
    /// Continental crust older than this is a craton (fully above `craton_age_ma + 100`), Ma.
    pub craton_age_ma: f64,
    /// Sediment thickness from which a continental cell counts as a basin (fully at twice), m.
    pub basin_min_m: f64,
    /// Depth of the regolith on gentle land slopes, m.
    pub regolith_m: f64,
    /// Extra depth of laterite under a tropical humid climate on flat land, m.
    pub laterite_m: f64,
    /// Mean annual temperature range (°C) and precipitation range (mm/yr) over which laterite
    /// sets in.
    pub laterite_temperature_c: [f64; 2],
    pub laterite_precipitation_mm: [f64; 2],
    /// Precipitation ranges (mm/yr) of the `humid` and `arid` contexts.
    pub humid_mm: [f64; 2],
    pub arid_mm: [f64; 2],
    /// River placer sources fade downstream over this distance, m.
    pub placer_reach_m: f64,
    /// Wavelength of the relative variations of the sedimentary cover, m.
    pub cover_noise_m: f64,
}

impl Default for GeologyConfig {
    fn default() -> Self {
        Self {
            sediment_max_m: 500.0,
            basin_high_m: 400.0,
            basin_fill: 0.8,
            shelf_fill: 0.7,
            rift_fill: 0.9,
            craton_cover: 0.35,
            pelagic_m_per_ma: 0.4,
            pelagic_max_m: 80.0,
            craton_age_ma: 1_900.0,
            basin_min_m: 120.0,
            regolith_m: 4.0,
            laterite_m: 18.0,
            laterite_temperature_c: [16.0, 22.0],
            laterite_precipitation_mm: [900.0, 1_600.0],
            humid_mm: [700.0, 1_400.0],
            arid_mm: [250.0, 550.0],
            placer_reach_m: 15_000.0,
            cover_noise_m: 8_000.0,
        }
    }
}

impl GeologyConfig {
    pub fn validate(&self) -> Result<(), String> {
        let range = |r: [f64; 2]| r[0] < r[1];
        if !(self.sediment_max_m > 0.0
            && self.basin_min_m > 0.0
            && self.regolith_m >= 0.0
            && self.laterite_m >= 0.0
            && self.placer_reach_m > 0.0
            && self.cover_noise_m > 0.0)
        {
            return Err("sediment_max_m, basin_min_m, placer_reach_m, cover_noise_m must be > 0, regolith_m and laterite_m >= 0".into());
        }
        if !(range(self.laterite_temperature_c)
            && range(self.laterite_precipitation_mm)
            && range(self.humid_mm)
            && range(self.arid_mm))
        {
            return Err("climate ranges must be increasing".into());
        }
        Ok(())
    }
}

/// Output of the geology stage.
pub struct GeologyOutput {
    /// Context weights per tectonic cell, indexed by [`Context::index`].
    pub weights: Vec<[f32; CONTEXTS]>,
    /// Dominant setting per tectonic cell ([`setting`]).
    pub setting: PeriodicGrid<u8>,
    /// Sedimentary cover, m.
    pub sediment: PeriodicGrid<f32>,
    /// Weathering depth (regolith or laterite) on land, m.
    pub weathering: PeriodicGrid<f32>,
    /// Share of laterite in the weathering, 0..1.
    pub laterite: PeriodicGrid<f32>,
    /// Placer source strength of the rivers on the fine grid, 0 off rivers.
    pub river_source: PeriodicGrid<f32>,
}

impl GeologyOutput {
    /// Weight of `c` at tectonic cell `k`.
    #[inline]
    pub fn weight(&self, k: usize, c: Context) -> f64 {
        f64::from(self.weights[k][c.index()])
    }

    pub fn layers(&self) -> Vec<Layer> {
        vec![
            Layer::from_u8(
                "geo_context",
                "code (see geology::setting::NAMES)",
                &self.setting,
            ),
            Layer::from_f32("sediment_thickness", "m", &self.sediment),
            Layer::from_f32("weathering_depth", "m", &self.weathering),
            Layer::from_f32("laterite", "share", &self.laterite),
            Layer::from_f32("placer_source", "share", &self.river_source),
        ]
    }
}

#[inline]
pub fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Spreads a 0/1 field over neighbouring cells (two box blurs of radius 1), keeping cores at 1.
fn soften(v: &[f32], res: usize) -> Vec<f32> {
    let mut s = v.to_vec();
    crate::tectonics::box_blur(&mut s, res, res, 1);
    crate::tectonics::box_blur(&mut s, res, res, 1);
    s.iter().map(|&x| (x * 2.0).min(1.0)).collect()
}

/// Runs the geology stage.
pub fn run(
    cfg: &WorldConfig,
    tect: &TectonicOutput,
    ero: &ErosionOutput,
    clim: &ClimateOutput,
    progress: &crate::progress::Progress,
) -> GeologyOutput {
    let g = &cfg.geology;
    let res = tect.crust_type.width();
    let cell = tect.crust_type.cell_size();
    let n = res * res;
    let fine = &ero.elevation;
    let fres = fine.width();
    let ratio = fres / res;

    // Mean final elevation and sea share of each tectonic cell.
    let (elev, sea_share): (Vec<f64>, Vec<f64>) = (0..n)
        .into_par_iter()
        .map(|k| {
            let (i, j) = (k % res, k / res);
            let (mut s, mut sea) = (0.0, 0.0);
            for dj in 0..ratio {
                for di in 0..ratio {
                    let (fi, fj) = ((i * ratio + di) as i64, (j * ratio + dj) as i64);
                    s += f64::from(*fine.get(fi, fj));
                    sea += f64::from(u8::from(*ero.basin_id.get(fi, fj) == 0));
                }
            }
            let m = (ratio * ratio) as f64;
            (s / m, sea / m)
        })
        .unzip();
    progress.set(0.15);

    let flag = |bit: u8| -> Vec<f32> {
        let raw: Vec<f32> = tect
            .history_flags
            .data()
            .iter()
            .map(|&f| f32::from(u8::from(f & bit != 0)))
            .collect();
        soften(&raw, res)
    };
    let orogen = flag(flags::OROGENY);
    let arc = flag(flags::ARC);
    let rift = flag(flags::RIFT);
    let hotspot = flag(flags::HOTSPOT);
    let ophiolite = flag(flags::OPHIOLITE);
    let trench = flag(flags::TRENCH);
    let ridge = flag(flags::RIDGE);
    let cont_raw: Vec<f32> = tect
        .crust_type
        .data()
        .iter()
        .map(|&c| f32::from(u8::from(c == CONTINENTAL)))
        .collect();
    let mut cont = cont_raw.clone();
    crate::tectonics::box_blur(&mut cont, res, res, 1);
    // Regional (smoothed) tectonic elevation: lowlands make basins.
    let mut regional: Vec<f32> = tect.elevation.data().to_vec();
    for _ in 0..2 {
        crate::tectonics::box_blur(&mut regional, res, res, 6);
    }
    let noise = Fbm::new(
        stage_u64(cfg.seed, "geology-cover"),
        g.cover_noise_m,
        3,
        0.5,
    );
    let temp = &clim.temperature.x0;
    let precip = &clim.precipitation.x0;

    // Sediments, weathering, climate contexts.
    let per_cell: Vec<(f64, f64, f64, f64, f64)> = (0..n)
        .into_par_iter()
        .map(|k| {
            let (i, j) = (k % res, k / res);
            let (x, z) = tect.crust_type.cell_center(i, j);
            let c = f64::from(cont_raw[k]);
            let sea = sea_share[k] > 0.5;
            let w = |v: &Vec<f32>| f64::from(v[k]);
            let age = f64::from(tect.crust_age.data()[k]);
            let craton = c * smoothstep(g.craton_age_ma, g.craton_age_ma + 100.0, age);
            let low = smoothstep(g.basin_high_m, 0.0, f64::from(regional[k]));
            let mut s = if c > 0.5 {
                let basin = g.basin_fill * low * (1.0 - (1.0 - g.craton_cover) * craton);
                let shelf = if sea { g.shelf_fill } else { 0.0 };
                basin.max(shelf).max(g.rift_fill * w(&rift)) * g.sediment_max_m
            } else {
                (g.pelagic_m_per_ma * age).min(g.pelagic_max_m)
            };
            // Deltas and fans: what erosion deposited.
            let deposited = (elev[k] - f64::from(tect.elevation.data()[k])).max(0.0);
            s = s.max(deposited);
            s *= (1.0 - 0.95 * w(&orogen)) * (1.0 - 0.7 * w(&arc)) * (1.0 - 0.9 * w(&hotspot));
            s *= 0.7 + 0.6 * (0.5 + 0.5 * noise.sample(x, z)).clamp(0.0, 1.0);
            let s = s.clamp(0.0, g.sediment_max_m);
            // Slope of the final surface (central differences on the cell means).
            let at = |di: i64, dj: i64| {
                let ii = (i as i64 + di).rem_euclid(res as i64) as usize;
                let jj = (j as i64 + dj).rem_euclid(res as i64) as usize;
                elev[jj * res + ii]
            };
            let slope = ((at(1, 0) - at(-1, 0)).hypot(at(0, 1) - at(0, -1))) / (2.0 * cell);
            let t = f64::from(*temp.get(i as i64, j as i64));
            let p = f64::from(*precip.get(i as i64, j as i64));
            let flat = 1.0 - smoothstep(0.08, 0.3, slope);
            let laterite = if sea {
                0.0
            } else {
                smoothstep(g.laterite_temperature_c[0], g.laterite_temperature_c[1], t)
                    * smoothstep(
                        g.laterite_precipitation_mm[0],
                        g.laterite_precipitation_mm[1],
                        p,
                    )
                    * flat
            };
            let weathering = if sea {
                0.0
            } else {
                g.regolith_m * (1.0 - smoothstep(0.15, 0.6, slope)) + g.laterite_m * laterite
            };
            (s, weathering, laterite, p, craton)
        })
        .collect();
    let mut sediment: Vec<f32> = per_cell.iter().map(|v| v.0 as f32).collect();
    crate::tectonics::box_blur(&mut sediment, res, res, 1);
    progress.set(0.4);

    // Rivers carrying placer minerals: sources in orogens and arcs, carried downstream.
    let river_source = river_sources(g, ero, &orogen, &arc, res);
    let mut river_ctx = vec![0f32; n];
    for (fk, &v) in river_source.data().iter().enumerate() {
        let (fi, fj) = (fk % fres, fk / fres);
        let k = (fj / ratio) * res + fi / ratio;
        river_ctx[k] = river_ctx[k].max(v);
    }
    progress.set(0.8);

    // Coasts: cells holding both land and sea (fine), softened.
    let coast_raw: Vec<f32> = sea_share
        .iter()
        .map(|&s| f32::from(u8::from(s > 0.04 && s < 0.96)))
        .collect();
    let coast = soften(&coast_raw, res);

    let weights: Vec<[f32; CONTEXTS]> = (0..n)
        .map(|k| {
            let (_, _, lat, p, craton) = per_cell[k];
            let c = f64::from(cont[k]);
            let sea = sea_share[k];
            let o = f64::from(orogen[k]);
            let a = f64::from(arc[k]);
            let mut w = [0f32; CONTEXTS];
            let mut set = |ctx: Context, v: f64| w[ctx.index()] = v.clamp(0.0, 1.0) as f32;
            set(Context::Ocean, 1.0 - c);
            set(Context::Ridge, f64::from(ridge[k]));
            set(Context::Trench, f64::from(trench[k]));
            set(Context::Arc, a);
            set(Context::Orogen, o);
            set(Context::Rift, f64::from(rift[k]));
            set(Context::Hotspot, f64::from(hotspot[k]));
            set(Context::Ophiolite, f64::from(ophiolite[k]));
            set(
                Context::Ultramafic,
                f64::from(ophiolite[k]).max(0.5 * f64::from(hotspot[k])),
            );
            let free = (1.0 - o) * (1.0 - a);
            set(Context::Craton, craton * free);
            set(Context::Platform, (c - craton).max(0.0) * free);
            set(
                Context::Basin,
                c * smoothstep(g.basin_min_m, 2.0 * g.basin_min_m, f64::from(sediment[k])),
            );
            let water = (-elev[k]).max(0.0);
            set(
                Context::MarineShelf,
                c * smoothstep(0.3, 0.7, sea) * smoothstep(400.0, 150.0, water),
            );
            set(Context::Humid, smoothstep(g.humid_mm[0], g.humid_mm[1], p));
            set(
                Context::Arid,
                1.0 - smoothstep(g.arid_mm[0], g.arid_mm[1], p),
            );
            set(Context::Laterite, lat);
            set(Context::RiverSource, f64::from(river_ctx[k]));
            set(Context::Coast, f64::from(coast[k]));
            w
        })
        .collect();

    let setting: Vec<u8> = (0..n)
        .map(|k| {
            let w = &weights[k];
            let at = |c: Context| w[c.index()];
            let ordered = [
                (Context::Ophiolite, setting::OPHIOLITE),
                (Context::Hotspot, setting::HOTSPOT),
                // An arc caught in a collision is an orogen.
                (Context::Orogen, setting::OROGEN),
                (Context::Arc, setting::ARC),
                (Context::Rift, setting::RIFT),
            ];
            for (c, code) in ordered {
                if at(c) >= 0.5 {
                    return code;
                }
            }
            if cont_raw[k] < 0.5 {
                if at(Context::Ridge) >= 0.5 {
                    setting::RIDGE
                } else if at(Context::Trench) >= 0.5 {
                    setting::TRENCH
                } else {
                    setting::OCEAN
                }
            } else if at(Context::MarineShelf) >= 0.5 {
                setting::MARINE_SHELF
            } else if at(Context::Basin) >= 0.5 {
                setting::BASIN
            } else if at(Context::Craton) >= 0.5 {
                setting::CRATON
            } else {
                setting::PLATFORM
            }
        })
        .collect();
    progress.set(1.0);

    let grid = |v: Vec<f32>| PeriodicGrid::from_vec(res, res, cell, v);
    GeologyOutput {
        weights,
        setting: PeriodicGrid::from_vec(res, res, cell, setting),
        sediment: grid(sediment),
        weathering: grid(per_cell.iter().map(|v| v.1 as f32).collect()),
        laterite: grid(per_cell.iter().map(|v| v.2 as f32).collect()),
        river_source,
    }
}

/// Placer source strength of every river cell of the fine grid: the strongest orogen or arc
/// weight upstream, fading with `exp(−distance / placer_reach_m)` downstream.
fn river_sources(
    g: &GeologyConfig,
    ero: &ErosionOutput,
    orogen: &[f32],
    arc: &[f32],
    res: usize,
) -> PeriodicGrid<f32> {
    let dir = &ero.flow_direction;
    let fres = dir.width();
    let cell = dir.cell_size();
    let ratio = fres / res;
    let n = fres * fres;
    let target = |k: usize| -> Option<usize> {
        let d = dir.data()[k];
        if d == NO_DIR {
            return None;
        }
        let (di, dj) = D8[usize::from(d)];
        let (i, j) = ((k % fres) as i64, (k / fres) as i64);
        Some(dir.index(i + di, j + dj))
    };
    // Own source, then Kahn's order along the flow (deterministic: index order).
    let mut s: Vec<f32> = (0..n)
        .map(|k| {
            let (i, j) = (k % fres, k / fres);
            let t = (j / ratio) * res + i / ratio;
            orogen[t].max(arc[t])
        })
        .collect();
    let mut indegree = vec![0u8; n];
    for k in 0..n {
        if let Some(t) = target(k) {
            indegree[t] += 1;
        }
    }
    let mut queue: Vec<usize> = (0..n).filter(|&k| indegree[k] == 0).collect();
    let mut head = 0;
    while head < queue.len() {
        let k = queue[head];
        head += 1;
        if let Some(t) = target(k) {
            let (di, dj) = D8[usize::from(dir.data()[k])];
            let step = if di != 0 && dj != 0 {
                cell * std::f64::consts::SQRT_2
            } else {
                cell
            };
            let carried = s[k] * (-step / g.placer_reach_m).exp() as f32;
            s[t] = s[t].max(carried);
            indegree[t] -= 1;
            if indegree[t] == 0 {
                queue.push(t);
            }
        }
    }
    let rivers = ero.river_order.data();
    let data = (0..n)
        .map(|k| if rivers[k] > 0 { s[k] } else { 0.0 })
        .collect();
    PeriodicGrid::from_vec(fres, fres, cell, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_names_round_trip() {
        for (i, c) in Context::ALL.iter().enumerate() {
            assert_eq!(c.index(), i);
            assert_eq!(Context::from_name(c.name()), Some(*c));
        }
        assert_eq!(Context::from_name("nowhere"), None);
        assert!(GeologyConfig::default().validate().is_ok());
    }
}
