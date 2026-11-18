//! Ore deposits (spec §4.6): generation of the bodies. The catalogue, the bodies and their index
//! live in `nascio_world::deposits`.

pub use nascio_world::deposits::*;

use nascio_core::fmath::DetMath;
use std::collections::BTreeMap;

use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;

use crate::erosion::ErosionOutput;
use crate::erosion::flow::{D8, NO_DIR};
use crate::geology::{CONTEXTS, GeologyOutput};
use crate::grid::PeriodicGrid;
use crate::rng::stage_rng;
use crate::world_io::Layer;
use nascio_core::GeothermConfig;
use nascio_core::torus;

/// What sets the rock temperature of a column (spec §4.6): the Moho, the mean annual air
/// temperature and the geotherm.
#[derive(Clone, Copy)]
pub struct Heat<'a> {
    /// Moho depth below sea level, m (tectonic grid).
    pub moho_depth: &'a PeriodicGrid<f32>,
    /// Mean annual air temperature, °C.
    pub air_c: &'a PeriodicGrid<f32>,
    pub geotherm: GeothermConfig,
}

impl Heat<'_> {
    /// Depth below the rock surface `top` (m) of column `(x, z)` where the rock reaches `t` °C.
    pub fn depth_of(&self, x: f64, z: f64, top: f64, t: f64) -> f64 {
        let water = (-top).max(0.0);
        let ts = self
            .geotherm
            .surface_c(self.air_c.sample_bilinear(x, z), water);
        let crust = (top + self.moho_depth.sample_bilinear(x, z)).max(50.0);
        self.geotherm.depth_of(ts, crust, t)
    }
}

/// Everything the placement reads.
struct Inputs<'a> {
    geo: &'a GeologyOutput,
    ero: &'a ErosionOutput,
    heat: Heat<'a>,
    res: usize,
    cell: f64,
}

impl Inputs<'_> {
    fn cell_index(&self, x: f64, z: f64) -> usize {
        let i = ((torus::wrap_f64(x) / self.cell) as usize).min(self.res - 1);
        let j = ((torus::wrap_f64(z) / self.cell) as usize).min(self.res - 1);
        j * self.res + i
    }

    fn weights(&self, x: f64, z: f64) -> &[f32; CONTEXTS] {
        &self.geo.weights[self.cell_index(x, z)]
    }

    fn surface(&self, x: f64, z: f64) -> f64 {
        self.ero.elevation.sample_bilinear(x, z)
    }
}

fn uniform(rng: &mut ChaCha8Rng, r: [f64; 2]) -> f64 {
    r[0] + (r[1] - r[0]) * rng.random::<f64>()
}

/// Poisson draw (Knuth; `lambda` stays small per tile).
fn poisson(rng: &mut ChaCha8Rng, lambda: f64) -> usize {
    if lambda <= 0.0 {
        return 0;
    }
    let l = (-lambda.min(50.0)).det_exp();
    let mut k = 0;
    let mut p = 1.0;
    loop {
        p *= rng.random::<f64>();
        if p <= l || k >= 200 {
            return k;
        }
        k += 1;
    }
}

/// Moves a placer onto the nearest river cell (river placers) or beach cell (coastal ones)
/// within ~160 m; returns the new centre and the direction of the band.
fn snap_placer(inp: &Inputs, x: f64, z: f64, river: bool) -> Option<(f64, f64, f64)> {
    let e = &inp.ero.elevation;
    let fcell = e.cell_size();
    let (ci, cj) = e.cell_of(x, z);
    let reach = (160.0 / fcell).ceil().max(1.0) as i64;
    let mut best: Option<(f64, i64, i64)> = None;
    for dj in -reach..=reach {
        for di in -reach..=reach {
            let (i, j) = (ci as i64 + di, cj as i64 + dj);
            let k = e.index(i, j);
            let score = if river {
                let s = f64::from(inp.geo.river_source.data()[k]);
                if s <= 0.05 {
                    continue;
                }
                s
            } else {
                let h = f64::from(e.data()[k]);
                let land = *inp.ero.basin_id.get(i, j) != 0;
                if !land || !(0.0..4.0).contains(&h) {
                    continue;
                }
                1.0 / (1.0 + h)
            };
            let d = ((di * di + dj * dj) as f64).sqrt();
            let s = score / (1.0 + 0.2 * d);
            if best.is_none_or(|b| s > b.0) {
                best = Some((s, i, j));
            }
        }
    }
    let (_, i, j) = best?;
    let (px, pz) = (
        (i.rem_euclid(e.width() as i64) as f64 + 0.5) * fcell,
        (j.rem_euclid(e.width() as i64) as f64 + 0.5) * fcell,
    );
    let yaw = if river {
        let d = inp.ero.flow_direction.data()[e.index(i, j)];
        if d == NO_DIR {
            0.0
        } else {
            let (di, dj) = D8[usize::from(d)];
            (dj as f64).det_atan2(di as f64)
        }
    } else {
        // Along the coast: across the slope.
        let gx = e.sample_bilinear(px + fcell, pz) - e.sample_bilinear(px - fcell, pz);
        let gz = e.sample_bilinear(px, pz + fcell) - e.sample_bilinear(px, pz - fcell);
        gz.det_atan2(gx) + std::f64::consts::FRAC_PI_2
    };
    Some((px, pz, yaw))
}

/// Draws the bodies of type `kind` in tile `t`.
fn tile_bodies(seed: u64, kind: usize, d: &DepositType, t: usize, inp: &Inputs) -> Vec<Body> {
    if d.abundance <= 0.0 {
        return Vec::new();
    }
    let (ti, tj) = (t % TILES, t / TILES);
    let (x0, z0) = (ti as f64 * TILE_M, tj as f64 * TILE_M);
    // Mean and largest weight over an 8×8 sampling of the tile.
    let (mut mean, mut max) = (0.0f64, 0.0f64);
    for sj in 0..8 {
        for si in 0..8 {
            let x = x0 + (si as f64 + 0.5) * TILE_M / 8.0;
            let z = z0 + (sj as f64 + 0.5) * TILE_M / 8.0;
            let w = d.weight(inp.weights(x, z));
            mean += w / 64.0;
            max = max.max(w);
        }
    }
    if mean <= 0.0 {
        return Vec::new();
    }
    let mut rng = stage_rng(seed, &format!("deposits/{}/{t}", d.name));
    let area_km2 = TILE_M * TILE_M / 1e6;
    let count = poisson(&mut rng, d.abundance * area_km2 / 100.0 * mean);
    let mut out = Vec::new();
    for _ in 0..count {
        // Position: rejection sampling on the weight inside the tile.
        let mut pos = None;
        for _ in 0..16 {
            let x = x0 + rng.random::<f64>() * TILE_M;
            let z = z0 + rng.random::<f64>() * TILE_M;
            if rng.random::<f64>() * max < d.weight(inp.weights(x, z)) {
                pos = Some((x, z));
                break;
            }
        }
        if let Some(b) = shape_body(kind, d, &mut rng, pos, inp) {
            out.push(b);
        }
    }
    out
}

/// Draws the size, depth and orientation of a body of type `kind` at `pos` and places it in
/// its column; `None` without a position or without room for it. The draws come first, in the
/// same order whatever the outcome.
fn shape_body(
    kind: usize,
    d: &DepositType,
    rng: &mut ChaCha8Rng,
    pos: Option<(f64, f64)>,
    inp: &Inputs,
) -> Option<Body> {
    let size = uniform(rng, d.size_m);
    let thick = uniform(rng, d.thickness_m);
    let mut depth = uniform(rng, d.depth_m);
    let aspect = 0.45 + 0.55 * rng.random::<f64>();
    let mut yaw = rng.random::<f64>() * std::f64::consts::TAU;
    let dip = (45.0 + 40.0 * rng.random::<f64>()).to_radians();
    let grade = (d.grade * (0.75 + 0.5 * rng.random::<f64>())).min(1.0);
    let body_seed = rng.random::<u32>();
    let (mut x, mut z) = pos?;
    let k = inp.cell_index(x, z);
    let sediment = f64::from(inp.geo.sediment.data()[k]);
    let weathering = f64::from(inp.geo.weathering.data()[k]);
    let (a, b, c) = match d.shape {
        Shape::Vein => (size / 2.0, size * (0.2 + 0.25 * aspect), thick / 2.0),
        Shape::Stock | Shape::Lens => (size / 2.0, size / 2.0 * aspect, thick / 2.0),
        Shape::Layer | Shape::Blanket | Shape::Trap => {
            (size / 2.0, size / 2.0 * aspect, thick / 2.0)
        }
        Shape::Placer => (size / 2.0, size / 2.0 * (0.12 + 0.1 * aspect), thick / 2.0),
    };
    let surface = inp.surface(x, z);
    if let Some(t_max) = d.max_temperature_c
        && !d.shape.follows_surface()
    {
        // Same draw, range shrunk to the rock cooler than `t_max` (top of the body).
        let limit = inp.heat.depth_of(x, z, surface, t_max) + c;
        if limit <= d.depth_m[0] {
            return None;
        }
        let hi = d.depth_m[1].min(limit);
        let span = (d.depth_m[1] - d.depth_m[0]).max(1e-9);
        depth = d.depth_m[0] + (depth - d.depth_m[0]) / span * (hi - d.depth_m[0]);
    }
    let y = match d.shape {
        Shape::Layer if d.host == Host::Sediment => {
            // Inside the sedimentary cover.
            let room = sediment - 2.0 * c;
            if room <= d.depth_m[0] {
                return None;
            }
            surface
                - (c + d.depth_m[0]
                    + rng.random::<f64>() * (room - d.depth_m[0]).min(d.depth_m[1] - d.depth_m[0]))
        }
        Shape::Layer => surface - sediment - depth - c,
        Shape::Blanket => {
            if weathering < 2.0 * c.min(1.0) {
                return None;
            }
            depth.min((weathering - c).max(c))
        }
        Shape::Placer => {
            let river = d.contexts.contains_key("river_source")
                || d.requires.iter().any(|r| r == "river_source");
            let (px, pz, py) = snap_placer(inp, x, z, river)?;
            (x, z, yaw) = (px, pz, py);
            depth.max(c)
        }
        _ => surface - depth,
    };
    Some(Body {
        kind: kind as u16,
        shape: d.shape,
        x: torus::wrap_f64(x),
        z: torus::wrap_f64(z),
        y,
        a: a as f32,
        b: b as f32,
        c: c as f32,
        yaw: yaw as f32,
        dip: dip as f32,
        grade: grade as f32,
        seed: body_seed,
    })
}

/// Draws the bodies of type `kind`, placed above the bodies `parents` of another type: one per
/// parent with the probability `d.probability` × the weight of `d` at the parent, centred on
/// it. Each parent has its own RNG (its rank among the parents).
fn above_bodies(
    seed: u64,
    kind: usize,
    d: &DepositType,
    parents: &[Body],
    inp: &Inputs,
) -> Vec<Body> {
    let p = d.probability.unwrap_or(0.0);
    parents
        .par_iter()
        .enumerate()
        .map(|(i, parent)| {
            let mut rng = stage_rng(seed, &format!("deposits/{}/above/{i}", d.name));
            let w = d.weight(inp.weights(parent.x, parent.z));
            let draw = rng.random::<f64>();
            let pos = (draw < (p * w).min(1.0)).then_some((parent.x, parent.z));
            shape_body(kind, d, &mut rng, pos, inp)
        })
        .collect::<Vec<_>>()
        .into_iter()
        .flatten()
        .collect()
}

/// Summary of the placement.
#[derive(Clone, Debug, Default)]
pub struct DepositsReport {
    /// Per type: `(name, bodies, mineralised volume m³, mean depth of the centre m)`.
    pub per_type: Vec<(String, usize, f64, f64)>,
}

impl std::fmt::Display for DepositsReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "deposits:")?;
        for (name, n, v, d) in &self.per_type {
            writeln!(
                f,
                "  {name:<16} {n:>5} bodies, {:>9.3} Mm3 ore, mean depth {d:>4.0} m",
                v / 1e6
            )?;
        }
        Ok(())
    }
}

/// Output of the deposit stage.
pub struct DepositsOutput {
    pub bodies: Vec<Body>,
    /// Dominant deposit (type index + 1) per tectonic cell, 0 = none.
    pub dominant: PeriodicGrid<u8>,
    /// Map-only types, as bits: `1 << 2k` province, `1 << (2k + 1)` trap of the `k`-th map-only
    /// type (provinces and traps of several types overlap).
    pub traps: PeriodicGrid<u8>,
    pub report: DepositsReport,
}

impl DepositsOutput {
    pub fn layers(&self) -> Vec<Layer> {
        vec![
            Layer::from_u8(
                "deposits",
                "deposit type + 1 (deposits.toml order), 0=none",
                &self.dominant,
            ),
            Layer::from_u8(
                "traps",
                "bits: 1<<2k province, 1<<(2k+1) trap of the k-th map-only type",
                &self.traps,
            ),
        ]
    }
}

/// Places the deposit bodies of `catalog` and builds the map layers.
pub fn run(
    seed: u64,
    catalog: &Catalog,
    geo: &GeologyOutput,
    ero: &ErosionOutput,
    heat: Heat<'_>,
    progress: &crate::progress::Progress,
) -> DepositsOutput {
    let res = geo.setting.width();
    let cell = geo.setting.cell_size();
    let inp = Inputs {
        geo,
        ero,
        heat,
        res,
        cell,
    };
    let mut bodies: Vec<Body> = Vec::new();
    let nt = catalog.deposits.len().max(1);
    for (kind, d) in catalog.deposits.iter().enumerate() {
        if let Some(parent) = &d.above {
            // Validated: an earlier type, already placed.
            let pk = catalog
                .deposits
                .iter()
                .position(|o| &o.name == parent)
                .expect("validated parent") as u16;
            let parents: Vec<Body> = bodies.iter().filter(|b| b.kind == pk).copied().collect();
            let above = above_bodies(seed, kind, d, &parents, &inp);
            bodies.extend(above);
            progress.set_range(0.0, 0.8, kind, nt, 1.0);
            continue;
        }
        let per_tile: Vec<Vec<Body>> = (0..TILES * TILES)
            .into_par_iter()
            .map(|t| tile_bodies(seed, kind, d, t, &inp))
            .collect();
        bodies.extend(per_tile.into_iter().flatten());
        progress.set_range(0.0, 0.8, kind, nt, 1.0);
    }

    // Map: dominant mineralised volume per cell over the footprints.
    let n = res * res;
    let mut score = vec![BTreeMap::<u16, f64>::new(); n];
    let mut traps = vec![0u8; n];
    let trap_rank: Vec<Option<u8>> = {
        let mut r = 0u8;
        catalog
            .deposits
            .iter()
            .map(|d| {
                (d.shape == Shape::Trap).then(|| {
                    r += 1;
                    r - 1
                })
            })
            .collect()
    };
    for (k, t) in traps.iter_mut().enumerate() {
        let w = &geo.weights[k];
        for (d, rank) in catalog.deposits.iter().zip(&trap_rank) {
            if let Some(r) = rank
                && d.weight(w) >= 0.3
            {
                *t |= 1 << (2 * r);
            }
        }
    }
    for b in &bodies {
        let r = b.radius() * 0.7;
        let reach = (r / cell).ceil() as i64;
        let (ci, cj) = ((b.x / cell) as i64, (b.z / cell) as i64);
        let vol = ore_volume(b);
        for dj in -reach..=reach {
            for di in -reach..=reach {
                let (cx, cz) = (
                    (ci + di) as f64 * cell + cell / 2.0,
                    (cj + dj) as f64 * cell + cell / 2.0,
                );
                if torus::distance_f64((b.x, b.z), (cx, cz)) > r.max(cell * 0.5) {
                    continue;
                }
                let k = geo.setting.index(ci + di, cj + dj);
                if b.shape == Shape::Trap {
                    if let Some(rk) = trap_rank[usize::from(b.kind)] {
                        traps[k] |= 1 << (2 * rk + 1);
                    }
                } else {
                    *score[k].entry(b.kind).or_default() += vol;
                }
            }
        }
    }
    let dominant: Vec<u8> = score
        .iter()
        .map(|s| {
            s.iter()
                .fold(None, |best: Option<(u16, f64)>, (&k, &v)| match best {
                    Some((_, bv)) if bv >= v => best,
                    _ => Some((k, v)),
                })
                .map_or(0, |(k, _)| (k + 1) as u8)
        })
        .collect();

    let mut report = DepositsReport::default();
    for (kind, d) in catalog.deposits.iter().enumerate() {
        let of: Vec<&Body> = bodies
            .iter()
            .filter(|b| usize::from(b.kind) == kind)
            .collect();
        let vol: f64 = of.iter().map(|b| ore_volume(b)).sum();
        let depth = if of.is_empty() {
            0.0
        } else {
            of.iter()
                .map(|b| {
                    if b.shape.follows_surface() {
                        b.y
                    } else {
                        inp.surface(b.x, b.z) - b.y
                    }
                })
                .sum::<f64>()
                / of.len() as f64
        };
        report.per_type.push((d.name.clone(), of.len(), vol, depth));
    }
    progress.set(1.0);
    let grid = |v: Vec<u8>| PeriodicGrid::from_vec(res, res, cell, v);
    DepositsOutput {
        bodies,
        dominant: grid(dominant),
        traps: grid(traps),
        report,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geology::Context;

    #[test]
    fn default_catalogue_is_valid_and_hashed() {
        let c = Catalog::default_catalog();
        assert!(c.deposits.len() >= 20);
        assert_eq!(c.hash().len(), 64);
        for name in [
            "lead_zinc",
            "laterite_nickel",
            "platinum",
            "manganese",
            "phosphate",
            "rock_salt",
            "ilmenite",
        ] {
            assert!(c.deposits.iter().any(|d| d.name == name), "{name}");
        }
        let pz = c.deposits.iter().find(|d| d.name == "lead_zinc").unwrap();
        assert!(pz.elements.iter().any(|e| e == "Ag"));
        let salt = c.deposits.iter().find(|d| d.name == "rock_salt").unwrap();
        assert_eq!(salt.cap.as_ref().unwrap().block, "sylvite");
        assert!(
            c.deposits
                .iter()
                .filter(|d| d.shape == Shape::Trap)
                .all(|d| d.blocks.is_empty())
        );
    }

    #[test]
    fn rejects_unknown_context_and_trap_blocks() {
        let base = r#"[[deposit]]
name = "x"
label = "X"
map_color = [1, 2, 3]
contexts = { nowhere = 1.0 }
shape = "lens"
blocks = [{ block = "xo", color = [1, 1, 1] }]
depth_m = [0, 10]
size_m = [10, 20]
thickness_m = [1, 2]
grade = 0.5
abundance = 1
"#;
        assert!(Catalog::parse(base).is_err());
        assert!(Catalog::parse(&base.replace("nowhere", "arc")).is_ok());
        assert!(
            Catalog::parse(
                &base
                    .replace("nowhere", "arc")
                    .replace("\"lens\"", "\"trap\"")
            )
            .is_err()
        );
    }

    fn body(shape: Shape, x: f64) -> Body {
        Body {
            kind: 0,
            shape,
            x,
            z: 500.0,
            y: -100.0,
            a: 60.0,
            b: 40.0,
            c: 20.0,
            yaw: 0.3,
            dip: 1.0,
            grade: 0.5,
            seed: 7,
        }
    }

    #[test]
    fn alternative_requirements_take_the_larger_weight() {
        let text = r#"[[deposit]]
name = "x"
label = "X"
map_color = [1, 2, 3]
contexts = { tropical = 1.0 }
requires = ["ultramafic|hotspot"]
shape = "blanket"
blocks = [{ block = "xo", color = [1, 1, 1] }]
depth_m = [1, 2]
size_m = [10, 20]
thickness_m = [1, 2]
grade = 0.5
abundance = 1
"#;
        let c = Catalog::parse(text).unwrap();
        let mut w = [0f32; CONTEXTS];
        w[Context::Tropical.index()] = 1.0;
        assert_eq!(c.deposits[0].weight(&w), 0.0);
        w[Context::Ultramafic.index()] = 0.5;
        w[Context::Hotspot.index()] = 0.8;
        assert!((c.deposits[0].weight(&w) - 0.8).abs() < 1e-6);
        assert!(Catalog::parse(&text.replace("hotspot\"", "nowhere\"")).is_err());
    }

    #[test]
    fn bodies_round_trip_and_cross_the_seam() {
        let bodies = vec![body(Shape::Stock, 10.0), body(Shape::Layer, 99_990.0)];
        let back = bodies_from_bytes(&bodies_to_bytes(&bodies)).unwrap();
        assert_eq!(back, bodies);
        // A body near x = 0 is found from both sides of the seam, with the same ore.
        let idx = BodyIndex::new(bodies);
        assert_eq!(idx.near(99_968.0, 480.0, 32.0).len(), 2);
        assert_eq!(idx.near(0.0, 480.0, 32.0).len(), 2);
        let s = &idx.bodies[0];
        assert!(s.ore_at(10.0, -100.0, 500.0, 0.0).is_some());
        assert_eq!(
            s.ore_at(-20.0, -100.0, 500.0, 0.0),
            s.ore_at(99_980.0, -100.0, 500.0, 0.0)
        );
        assert!(s.ore_at(10.0, -200.0, 500.0, 0.0).is_none());
        // Stock grade fades from the core.
        let core = s.ore_at(10.0, -100.0, 500.0, 0.0).unwrap().0;
        let rim = s.ore_at(10.0, -117.0, 500.0, 0.0).unwrap().0;
        assert!(core > rim);
    }

    #[test]
    fn surface_bodies_cross_the_seam() {
        // A blanket (depth below the ground) and a lens centred 10 m east of the seam: the
        // same ore on both sides of x = 0, whatever the side the block is reached from.
        let mut blanket = body(Shape::Blanket, 10.0);
        blanket.y = 4.0;
        blanket.c = 1.5;
        let lens = body(Shape::Lens, 10.0);
        for b in [blanket, lens] {
            let idx = BodyIndex::new(vec![b]);
            assert_eq!(idx.near(99_968.0, 480.0, 32.0).len(), 1, "{:?}", b.shape);
            assert_eq!(idx.near(0.0, 480.0, 32.0).len(), 1, "{:?}", b.shape);
            let (y, top) = if b.shape == Shape::Blanket {
                (96.0, 100.0)
            } else {
                (-100.0, 0.0)
            };
            let mut inside = 0;
            for dx in [-40.0, -20.0, -5.0, 5.0] {
                let east = b.ore_at(dx, y, 500.0, top);
                assert_eq!(
                    east,
                    b.ore_at(100_000.0 + dx, y, 500.0, top),
                    "{:?}",
                    b.shape
                );
                inside += usize::from(east.is_some());
            }
            assert!(
                inside >= 3,
                "{:?}: the body reaches across the seam",
                b.shape
            );
        }
    }

    #[test]
    fn blocks_by_share_and_cap() {
        let c = Catalog::default_catalog();
        let pc = c
            .deposits
            .iter()
            .find(|d| d.name == "porphyry_copper")
            .unwrap();
        assert_eq!(pick_block(pc, 0.9, 0.1), "chalcopyrite");
        assert_eq!(pick_block(pc, 0.9, 0.95), "molybdenite");
        let salt = c.deposits.iter().find(|d| d.name == "rock_salt").unwrap();
        assert_eq!(pick_block(salt, 0.8, 0.5), "sylvite");
        assert_eq!(pick_block(salt, 0.2, 0.5), "rock_salt");
        let h = block_hash(3, -5, 7, 99_999);
        assert!((0.0..1.0).contains(&h) && h == block_hash(3, -5, 7, 99_999));
    }
}
