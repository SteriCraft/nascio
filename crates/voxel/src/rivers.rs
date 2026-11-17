//! River water in the chunks: the rivers of the final hydrology as meandering splines, with a
//! game gauge, a channel carved under a surface that never rises downstream, flat banks and a
//! valley of bounded side slope (audit M7, anomalies b and d; decision of 2026-11-17).
//!
//! - **Network.** Water from `min_width_m` (physical width) on; downstream of a water cell
//!   every land cell keeps water down to its lake or the sea. Cells under permanent ice
//!   (`frozen`) carry no water: a river starts below its glacier.
//! - **Gauge.** Game gauge (default): width and depth by Strahler order of the water network
//!   (`order_width_m`, `order_depth_m`, the last entry for higher orders), so that rivers
//!   need bridges and carry boats whatever the size of the world. Physical gauge (option): the
//!   `river_width` and `river_depth` layers (hydraulic geometry, < 1 m deep in a 100 km world).
//! - **Surface.** The surface of a cell starts at its ground height, is capped by its upstream
//!   cells (never rises downstream) and is at least the level of the lake or sea it reaches;
//!   it is interpolated along the trace and rounded to whole blocks.
//! - **Trace.** Chains of cells follow the main stem (highest order, then widest). The cell
//!   centres, smoothed (`smooth_passes`), are joined by a Catmull-Rom spline; meanders displace
//!   it sideways by a sine of the arc length whose wavelength is `meander_wavelength` widths
//!   and whose amplitude is `meander_amplitude` wavelengths (at most `meander_max_amplitude`
//!   widths), fading out on steep reaches (gradient from `meander_flat` to `meander_steep`). A
//!   tributary ends exactly on the meandering main stem; its own meanders fade out over its
//!   last wavelength.
//! - **Profile.** Inside the channel the ground is lowered under the water (parabolic profile,
//!   at least one block); the banks (`bank_m`) are level with the water; beyond them the ground
//!   stays within `valley_slope` (m/m) of the water level up to `valley_height_m` above or
//!   below it: no vertical canyons, no dykes.

use nascio_core::consts::WORLD_SIZE_F64;
use nascio_core::fmath::DetMath;
use nascio_core::torus::{delta_f64, wrap_f64};
use nascio_world::erosion::flow::{D8, NO_DIR};
use nascio_world::grid::PeriodicGrid;
use nascio_world::rng::mix64;
use serde::{Deserialize, Serialize};

/// Width and depth of the rivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Gauge {
    /// By Strahler order (game gauge).
    Game,
    /// The hydraulic geometry of the world layers.
    Physical,
}

/// Parameters of the river water.
#[derive(Clone, Debug)]
pub struct RiverParams {
    pub min_width_m: f64,
    pub bank_m: f64,
    pub smooth_passes: usize,
    pub gauge: Gauge,
    pub min_order: u8,
    pub order_width_m: Vec<f64>,
    pub order_depth_m: Vec<f64>,
    pub meander_wavelength: f64,
    pub meander_angle_deg: f64,
    pub meander_max_amplitude: f64,
    pub meander_flat: f64,
    pub meander_steep: f64,
    pub valley_slope: f64,
    pub valley_height_m: f64,
}

/// One segment of the trace, from `a` to `a + d`.
#[derive(Clone, Copy, Debug)]
struct Seg {
    ax: f64,
    az: f64,
    dx: f32,
    dz: f32,
    /// Half width, depth (m) and water surface (m) at both ends.
    half: [f32; 2],
    depth: [f32; 2],
    level: [f32; 2],
}

/// The river at one column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RiverHit {
    /// In the channel: water fills `bed < y < water_top`.
    Channel { water_top: i32, bed: i32 },
    /// On a bank: the top block is `top`, level with the water.
    Bank { top: i32 },
    /// In the valley, `d` metres beyond the bank: the ground stays within `slope · d` of the
    /// bank height `top + 0.5`.
    Valley { top: i32, d: f64 },
}

/// Side of the spatial index cells, m (divides `WORLD_SIZE`).
const INDEX_CELL: f64 = 50.0;

/// The river traces of a world, with a spatial index.
#[derive(Clone, Debug)]
pub struct Rivers {
    segs: Vec<Seg>,
    /// `(cell, segment)` pairs, sorted: the segments that may reach each index cell.
    index: Vec<(u32, u32)>,
    res: usize,
    bank: f64,
    valley_r: f64,
    valley_slope: f64,
}

/// A point of a chain in coordinates unwrapped from the chain's first node.
#[derive(Clone, Copy, Debug)]
struct Node {
    x: f64,
    z: f64,
    half: f64,
    depth: f64,
    level: f64,
}

/// Lateral offset of a sine-generated curve (Langbein): the heading oscillates as
/// `angle · sin(2π s / M)` along the arc length, which gives the rounded loops of meanders.
/// Tabulated over one wavelength of the valley axis, normalised to a peak of 1.
struct MeanderShape {
    /// Peak lateral offset, in valley wavelengths.
    peak: f64,
    table: Vec<f64>,
}

impl MeanderShape {
    const N: usize = 256;

    fn new(angle: f64) -> Self {
        // Integrate the curve over one period, then resample it on the valley axis.
        let steps = 8_192;
        let mut pts = Vec::with_capacity(steps + 1);
        let (mut x, mut y) = (0.0f64, 0.0f64);
        pts.push((0.0, 0.0));
        let ds = 1.0 / steps as f64;
        for i in 0..steps {
            let s = (i as f64 + 0.5) * ds;
            let th = angle * (std::f64::consts::TAU * s).det_sin();
            x += th.det_cos() * ds;
            y += th.det_sin() * ds;
            pts.push((x, y));
        }
        let len = x;
        let mean = pts.iter().map(|p| p.1).sum::<f64>() / pts.len() as f64;
        let mut table = Vec::with_capacity(Self::N);
        let mut k = 0;
        for i in 0..Self::N {
            let xi = i as f64 / Self::N as f64 * len;
            while k + 1 < pts.len() && pts[k + 1].0 < xi {
                k += 1;
            }
            let (a, b) = (pts[k], pts[(k + 1).min(pts.len() - 1)]);
            let t = if b.0 > a.0 {
                (xi - a.0) / (b.0 - a.0)
            } else {
                0.0
            };
            table.push((a.1 + (b.1 - a.1) * t - mean) / len);
        }
        let peak = table.iter().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-9);
        table.iter_mut().for_each(|v| *v /= peak);
        Self { peak, table }
    }

    /// Normalised offset at `cycles` valley wavelengths (periodic).
    fn at(&self, cycles: f64) -> f64 {
        let u = cycles.rem_euclid(1.0) * Self::N as f64;
        let i = u.floor() as usize % Self::N;
        let t = u - u.floor();
        self.table[i] + (self.table[(i + 1) % Self::N] - self.table[i]) * t
    }
}

impl Rivers {
    /// Builds the traces from the hydrology layers (all on the elevation grid). `ground` is the
    /// eroded elevation; `lake_level` the lake surface per cell (`-∞` off lakes); `frozen`
    /// marks the cells under permanent ice.
    pub fn new(
        dir: &PeriodicGrid<u8>,
        width: &PeriodicGrid<f32>,
        depth: &PeriodicGrid<f32>,
        ground: &PeriodicGrid<f32>,
        lake_level: &PeriodicGrid<f32>,
        frozen: &[bool],
        p: &RiverParams,
    ) -> Self {
        let res = dir.width();
        let cell = dir.cell_size();
        let n = res * res;
        let d = dir.data();
        let lake = |k: usize| lake_level.data()[k];
        let step = |k: usize, code: u8| {
            let (i, j) = ((k % res) as i64, (k / res) as i64);
            let (di, dj) = D8[usize::from(code)];
            dir.index(i + di, j + dj)
        };
        let down = |k: usize| (d[k] != NO_DIR).then(|| step(k, d[k]));
        // Land river cells (not lake, not sea, not under ice).
        let land = |k: usize| d[k] != NO_DIR && !lake(k).is_finite() && !frozen[k];
        // Strahler order on the whole drainage network of the land (upstream first).
        let mut indeg = vec![0u8; n];
        for k in (0..n).filter(|&k| land(k)) {
            if let Some(m) = down(k).filter(|&m| land(m)) {
                indeg[m] += 1;
            }
        }
        let mut net: Vec<usize> = (0..n).filter(|&k| land(k) && indeg[k] == 0).collect();
        let mut head = 0;
        while head < net.len() {
            if let Some(m) = down(net[head]).filter(|&m| land(m)) {
                indeg[m] -= 1;
                if indeg[m] == 0 {
                    net.push(m);
                }
            }
            head += 1;
        }
        let mut strahler = vec![0u8; n];
        let mut top_in = vec![(0u8, 0u8); n];
        for &k in &net {
            let (o, c) = top_in[k];
            strahler[k] = match (o, c) {
                (0, _) => 1,
                (o, c) if c >= 2 => o.saturating_add(1),
                (o, _) => o,
            };
            if let Some(m) = down(k).filter(|&m| land(m)) {
                let s = strahler[k];
                let t = &mut top_in[m];
                if s > t.0 {
                    *t = (s, 1);
                } else if s == t.0 {
                    t.1 = t.1.saturating_add(1);
                }
            }
        }
        let min_w = p.min_width_m as f32;
        let wet = |k: usize| match p.gauge {
            Gauge::Game => strahler[k] >= p.min_order,
            Gauge::Physical => width.data()[k] >= min_w,
        };
        let mut water = vec![false; n];
        for k in 0..n {
            if !(land(k) && wet(k)) || water[k] {
                continue;
            }
            // Downstream of a water cell, the river keeps water down to its lake or the sea.
            let mut cur = k;
            loop {
                water[cur] = true;
                match down(cur) {
                    Some(m) if land(m) && !water[m] => cur = m,
                    _ => break,
                }
            }
        }
        let wdown = |k: usize| down(k).filter(|&m| water[m]);
        // Topological order, upstream first (Kahn, FIFO from the sources in index order).
        let mut indeg = vec![0u8; n];
        for k in (0..n).filter(|&k| water[k]) {
            if let Some(m) = wdown(k) {
                indeg[m] += 1;
            }
        }
        let mut order: Vec<usize> = (0..n).filter(|&k| water[k] && indeg[k] == 0).collect();
        let mut head = 0;
        while head < order.len() {
            if let Some(m) = wdown(order[head]) {
                indeg[m] -= 1;
                if indeg[m] == 0 {
                    order.push(m);
                }
            }
            head += 1;
        }
        // Surface: ground, capped from upstream, at least the level of the receiving water.
        let mut level: Vec<f32> = ground.data().to_vec();
        for &k in &order {
            if let Some(m) = wdown(k) {
                level[m] = level[m].min(level[k]);
            }
        }
        let mut end = vec![0.0f32; n];
        for &k in order.iter().rev() {
            end[k] = match down(k) {
                Some(m) if water[m] => end[m],
                Some(m) if lake(m).is_finite() => lake(m),
                _ => 0.0,
            };
            level[k] = level[k].max(end[k]);
        }
        let table = |v: &[f64], o: u8| {
            let i = usize::from(o).saturating_sub(usize::from(p.min_order));
            v[i.min(v.len() - 1)]
        };
        let phys_w = |k: usize| f64::from(width.data()[k].max(min_w));
        let (gw, gd): (Vec<f64>, Vec<f64>) = (0..n)
            .map(|k| {
                if !water[k] {
                    return (0.0, 0.0);
                }
                match p.gauge {
                    Gauge::Game => (
                        table(&p.order_width_m, strahler[k]),
                        table(&p.order_depth_m, strahler[k]),
                    ),
                    Gauge::Physical => (phys_w(k), f64::from(depth.data()[k].max(0.0))),
                }
            })
            .unzip();
        // Main upstream cell: highest order, then widest physically, then first in index order.
        let mut up = vec![u32::MAX; n];
        for k in (0..n).filter(|&k| water[k]) {
            if let Some(m) = wdown(k) {
                let better = up[m] == u32::MAX || {
                    let u = up[m] as usize;
                    (strahler[k], phys_w(k)) > (strahler[u], phys_w(u))
                };
                if better {
                    up[m] = k as u32;
                }
            }
        }
        // Offsets from the cell centres, smoothed towards the upstream and downstream cells.
        let offset_to = |k: usize, m: usize| {
            let (ki, kj) = ((k % res) as f64, (k / res) as f64);
            let (mi, mj) = ((m % res) as f64, (m / res) as f64);
            let r = res as f64;
            let dw = |a: f64, b: f64| {
                let v = (b - a).rem_euclid(r);
                if v > r / 2.0 { v - r } else { v }
            };
            (dw(ki, mi) * cell, dw(kj, mj) * cell)
        };
        let mut off = vec![(0.0f64, 0.0f64); n];
        for _ in 0..p.smooth_passes {
            let prev = off.clone();
            for &k in &order {
                let (Some(m), u) = (wdown(k), up[k]) else {
                    continue;
                };
                if u == u32::MAX {
                    continue;
                }
                let u = u as usize;
                let (ux, uz) = offset_to(k, u);
                let (mx, mz) = offset_to(k, m);
                off[k] = (
                    0.5 * prev[k].0 + 0.25 * (ux + prev[u].0) + 0.25 * (mx + prev[m].0),
                    0.5 * prev[k].1 + 0.25 * (uz + prev[u].1) + 0.25 * (mz + prev[m].1),
                );
            }
        }
        let centre = |k: usize| {
            let (cx, cz) = dir.cell_center(k % res, k / res);
            (wrap_f64(cx + off[k].0), wrap_f64(cz + off[k].1))
        };

        // Chains along the main stems, from each source (index order).
        let mut chain_of = vec![u32::MAX; n];
        let mut chains: Vec<Vec<usize>> = Vec::new();
        for &k in order.iter().filter(|&&k| up[k] == u32::MAX) {
            let mut c = vec![k];
            let mut cur = k;
            while let Some(m) = wdown(cur).filter(|&m| up[m] as usize == cur) {
                c.push(m);
                cur = m;
            }
            for &q in &c {
                chain_of[q] = chains.len() as u32;
            }
            chains.push(c);
        }
        // A tributary is laid out after the chain it joins (hops to the mouth, then index).
        let junction = |c: &[usize]| wdown(*c.last().expect("non-empty chain"));
        let mut hops = vec![u32::MAX; chains.len()];
        for i in 0..chains.len() {
            let mut path = vec![i];
            let mut h = 0;
            let mut cur = i;
            while let Some(m) = junction(&chains[cur]) {
                let next = chain_of[m] as usize;
                if hops[next] != u32::MAX {
                    h = hops[next] + 1;
                    break;
                }
                path.push(next);
                cur = next;
            }
            for (a, &c) in path.iter().rev().enumerate() {
                hops[c] = h + a as u32;
            }
        }
        let mut sequence: Vec<usize> = (0..chains.len()).collect();
        sequence.sort_by_key(|&c| (hops[c], c));

        let shape = MeanderShape::new(p.meander_angle_deg.to_radians());
        // Displaced (meandering) position of every chain node, absolute.
        let mut placed: Vec<Option<(f64, f64)>> = vec![None; n];
        let mut segs = Vec::new();
        let tau = std::f64::consts::TAU;
        for &ci in &sequence {
            let c = &chains[ci];
            let (x0, z0) = centre(c[0]);
            let node = |k: usize, x: f64, z: f64, lvl: f32| Node {
                x,
                z,
                half: 0.5 * gw[k],
                depth: gd[k],
                level: f64::from(lvl),
            };
            let mut pts = vec![node(c[0], 0.0, 0.0, level[c[0]])];
            for w in c.windows(2) {
                let (k, m) = (w[0], w[1]);
                let last = *pts.last().expect("node");
                let (mx, mz) = offset_to(k, m);
                let (sx, sz) = (mx + off[m].0 - off[k].0, mz + off[m].1 - off[k].1);
                pts.push(node(m, last.x + sx, last.z + sz, level[m]));
            }
            let kn = *c.last().expect("node");
            let lastp = *pts.last().expect("node");
            let joins = junction(c);
            match (joins, down(kn)) {
                (Some(m), _) => {
                    let (px, pz) = placed[m].unwrap_or_else(|| centre(m));
                    let (ax, az) = (wrap_f64(x0 + lastp.x), wrap_f64(z0 + lastp.z));
                    pts.push(Node {
                        x: lastp.x + delta_f64(ax, px),
                        z: lastp.z + delta_f64(az, pz),
                        ..node(m, 0.0, 0.0, level[m])
                    });
                }
                (None, Some(m)) => {
                    let (mx, mz) = offset_to(kn, m);
                    pts.push(Node {
                        x: lastp.x + mx - off[kn].0,
                        z: lastp.z + mz - off[kn].1,
                        ..node(kn, 0.0, 0.0, end[kn])
                    });
                }
                (None, None) => {}
            }
            // Catmull-Rom samples of the base curve, with the attributes interpolated.
            let mut samples: Vec<(Node, Option<usize>)> = Vec::new();
            let np = pts.len();
            for i in 0..np.saturating_sub(1) {
                let p1 = pts[i];
                let p2 = pts[i + 1];
                let p0 = if i > 0 { pts[i - 1] } else { p1 };
                let p3 = if i + 2 < np { pts[i + 2] } else { p2 };
                let len = (p2.x - p1.x).det_hypot(p2.z - p1.z);
                let lambda = p.meander_wavelength * 2.0 * p1.half.max(0.5);
                let ds = (lambda / 16.0).clamp(3.0, 40.0);
                let pieces = (len / ds).ceil().max(1.0) as usize;
                for s in 0..pieces {
                    let t = s as f64 / pieces as f64;
                    let cr = |a: f64, b: f64, cc: f64, dd: f64| {
                        let (t2, t3) = (t * t, t * t * t);
                        0.5 * (2.0 * b
                            + (cc - a) * t
                            + (2.0 * a - 5.0 * b + 4.0 * cc - dd) * t2
                            + (3.0 * b - a - 3.0 * cc + dd) * t3)
                    };
                    let l = |a: f64, b: f64| a + (b - a) * t;
                    samples.push((
                        Node {
                            x: cr(p0.x, p1.x, p2.x, p3.x),
                            z: cr(p0.z, p1.z, p2.z, p3.z),
                            half: l(p1.half, p2.half),
                            depth: l(p1.depth, p2.depth),
                            level: l(p1.level, p2.level),
                        },
                        (s == 0 && i < c.len()).then(|| c[i]),
                    ));
                }
            }
            if let Some(&last) = pts.last() {
                samples.push((last, None));
            }
            if samples.len() < 2 {
                continue;
            }
            let h = mix64(0x6D65_616E ^ c[0] as u64);
            let phi0 = (h & 0xFFFF) as f64 / 65_536.0;
            let phi1 = ((h >> 16) & 0xFFFF) as f64 / 65_536.0 * tau;
            // Arc length and meander phase.
            let mut s_at = vec![0.0f64; samples.len()];
            let mut phase = vec![0.0f64; samples.len()];
            for j in 1..samples.len() {
                let (a, b) = (samples[j - 1].0, samples[j].0);
                let ds = (b.x - a.x).det_hypot(b.z - a.z);
                s_at[j] = s_at[j - 1] + ds;
                let lambda = p.meander_wavelength * 2.0 * a.half.max(0.5);
                // Cycles along the valley, with a slightly irregular wavelength.
                let jitter = 1.0 + 0.15 * (tau * 0.31 * phase[j - 1] + phi1).det_sin();
                phase[j] = phase[j - 1] + jitter * ds / lambda;
            }
            let total = *s_at.last().expect("samples");
            // River gradient averaged over one meander wavelength (two pointers).
            let mut grad = vec![0.0f64; samples.len()];
            let (mut lo, mut hi) = (0usize, 0usize);
            for j in 0..samples.len() {
                let half_l = 0.5 * p.meander_wavelength * 2.0 * samples[j].0.half.max(0.5);
                while s_at[lo] < s_at[j] - half_l {
                    lo += 1;
                }
                while hi + 1 < samples.len() && s_at[hi + 1] <= s_at[j] + half_l {
                    hi += 1;
                }
                let span = s_at[hi] - s_at[lo];
                if span > 1e-6 {
                    grad[j] = (samples[lo].0.level - samples[hi].0.level).max(0.0) / span;
                }
            }
            let last_j = samples.len() - 1;
            let disp: Vec<(f64, f64)> = (0..samples.len())
                .map(|j| {
                    let (q, _) = samples[j];
                    let (a, b) = (
                        samples[j.saturating_sub(1)].0,
                        samples[(j + 1).min(last_j)].0,
                    );
                    let (tx, tz) = (b.x - a.x, b.z - a.z);
                    let tl = tx.det_hypot(tz);
                    if tl < 1e-9 {
                        return (q.x, q.z);
                    }
                    let width = 2.0 * q.half.max(0.5);
                    let lambda = p.meander_wavelength * width;
                    let flat = 1.0
                        - nascio_core::math::smoothstep(p.meander_flat, p.meander_steep, grad[j]);
                    let taper = if joins.is_some() {
                        nascio_core::math::smoothstep(0.0, lambda, total - s_at[j])
                    } else {
                        1.0
                    };
                    let amp = (shape.peak * lambda).min(p.meander_max_amplitude * width)
                        * flat
                        * taper
                        * (0.8 + 0.2 * (tau * 0.13 * phase[j] + phi1).det_sin());
                    let o = amp * shape.at(phase[j] + phi0);
                    (q.x - tz / tl * o, q.z + tx / tl * o)
                })
                .collect();
            for (j, (_, k)) in samples.iter().enumerate() {
                if let Some(k) = *k {
                    placed[k] = Some((wrap_f64(x0 + disp[j].0), wrap_f64(z0 + disp[j].1)));
                }
            }
            for j in 0..last_j {
                let (a, b) = (samples[j].0, samples[j + 1].0);
                let f = |v: f64| v as f32;
                segs.push(Seg {
                    ax: wrap_f64(x0 + disp[j].0),
                    az: wrap_f64(z0 + disp[j].1),
                    dx: f(disp[j + 1].0 - disp[j].0),
                    dz: f(disp[j + 1].1 - disp[j].1),
                    half: [f(a.half), f(b.half)],
                    depth: [f(a.depth), f(b.depth)],
                    level: [f(a.level), f(b.level)],
                });
            }
        }
        let valley_r = if p.valley_slope > 0.0 {
            p.valley_height_m / p.valley_slope
        } else {
            0.0
        };
        let ires = (WORLD_SIZE_F64 / INDEX_CELL).round() as usize;
        let mut index = Vec::new();
        for (s, g) in segs.iter().enumerate() {
            let r = f64::from(g.half[0].max(g.half[1])) + p.bank_m + valley_r + 1.0;
            let (x0, x1) = (
                g.ax + f64::from(g.dx.min(0.0)),
                g.ax + f64::from(g.dx.max(0.0)),
            );
            let (z0, z1) = (
                g.az + f64::from(g.dz.min(0.0)),
                g.az + f64::from(g.dz.max(0.0)),
            );
            let ci = |v: f64| (v / INDEX_CELL).floor() as i64;
            let wrap_i = |v: i64| v.rem_euclid(ires as i64) as usize;
            for j in ci(z0 - r)..=ci(z1 + r) {
                for i in ci(x0 - r)..=ci(x1 + r) {
                    index.push(((wrap_i(j) * ires + wrap_i(i)) as u32, s as u32));
                }
            }
        }
        index.sort_unstable();
        index.dedup();
        Self {
            segs,
            index,
            res: ires,
            bank: p.bank_m,
            valley_r,
            valley_slope: p.valley_slope,
        }
    }

    /// Number of trace segments.
    pub fn len(&self) -> usize {
        self.segs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    /// Maximum side slope of the valleys, m/m.
    pub fn valley_slope(&self) -> f64 {
        self.valley_slope
    }

    /// The river at world position `(x, z)` (column centre), if any.
    pub fn at(&self, x: f64, z: f64) -> Option<RiverHit> {
        let i = (wrap_f64(x) / INDEX_CELL).floor() as usize % self.res;
        let j = (wrap_f64(z) / INDEX_CELL).floor() as usize % self.res;
        let key = (j * self.res + i) as u32;
        let lo = self.index.partition_point(|e| e.0 < key);
        let mut best: Option<(f64, &Seg, f64, f64)> = None;
        for &(c, s) in &self.index[lo..] {
            if c != key {
                break;
            }
            let g = &self.segs[s as usize];
            let (px, pz) = (delta_f64(g.ax, x), delta_f64(g.az, z));
            let (dx, dz) = (f64::from(g.dx), f64::from(g.dz));
            let len2 = dx * dx + dz * dz;
            let t = if len2 > 0.0 {
                ((px * dx + pz * dz) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (ex, ez) = (px - t * dx, pz - t * dz);
            let half = f64::from(g.half[0]) + (f64::from(g.half[1]) - f64::from(g.half[0])) * t;
            // Cheap reject before the square root.
            let reach = half + self.bank + self.valley_r;
            if ex * ex + ez * ez > reach * reach {
                continue;
            }
            let dist = ex.det_hypot(ez);
            let e = dist - half;
            if best.is_none_or(|b| e < b.0) {
                best = Some((e, g, t, dist / half.max(1e-3)));
            }
        }
        let (e, g, t, frac) = best?;
        if e > self.bank + self.valley_r {
            return None;
        }
        let lerp = |v: [f32; 2]| f64::from(v[0]) + (f64::from(v[1]) - f64::from(v[0])) * t;
        let water_top = lerp(g.level).round() as i32;
        if e > self.bank {
            return Some(RiverHit::Valley {
                top: water_top - 1,
                d: e - self.bank,
            });
        }
        if e > 0.0 {
            return Some(RiverHit::Bank { top: water_top - 1 });
        }
        let blocks = (lerp(g.depth) * (1.0 - frac * frac)).ceil().max(1.0) as i32;
        Some(RiverHit::Channel {
            water_top,
            bed: water_top - 1 - blocks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(angle: f64) -> RiverParams {
        RiverParams {
            min_width_m: 1.5,
            bank_m: 2.0,
            smooth_passes: 2,
            gauge: Gauge::Game,
            min_order: 1,
            order_width_m: vec![6.0, 10.0],
            order_depth_m: vec![2.0, 3.0],
            meander_wavelength: 12.0,
            meander_angle_deg: angle,
            meander_max_amplitude: 3.0,
            meander_flat: 0.005,
            meander_steep: 0.03,
            valley_slope: 0.6,
            valley_height_m: 12.0,
        }
    }

    /// A straight river along +x on row 0 of a 32 m grid (crossing the x seam), falling
    /// `fall` m per cell from x = 2000 m to the sea, with (`pit`) a pit that must not raise the
    /// surface downstream.
    fn straight(p: &RiverParams, fall: f32, pit: bool) -> Rivers {
        let res = (WORLD_SIZE_F64 / 32.0) as usize;
        let cell = 32.0;
        let len = 70usize;
        let start = res - 40;
        let on = |i: usize| (i + res - start) % res < len;
        let dir = PeriodicGrid::from_fn(
            res,
            res,
            cell,
            |i, j| {
                if j == 0 && on(i) { 0 } else { NO_DIR }
            },
        );
        let width = PeriodicGrid::from_fn(
            res,
            res,
            cell,
            |i, j| {
                if j == 0 && on(i) { 4.0 } else { 0.0 }
            },
        );
        let depth = width.map(|&w| w / 2.0);
        let ground = PeriodicGrid::from_fn(res, res, cell, |i, j| {
            let s = (i + res - start) % res;
            if j == 0 && on(i) {
                if pit && s == 30 {
                    5.0
                } else {
                    10.0 + fall * (70 - s) as f32
                }
            } else {
                -5.0
            }
        });
        Rivers::new(
            &dir,
            &width,
            &depth,
            &ground,
            &ground.map(|_| f32::NEG_INFINITY),
            &vec![false; res * res],
            p,
        )
    }

    #[test]
    fn surface_never_rises_downstream_and_crosses_the_seam() {
        let r = straight(&params(0.0), 1.0, true);
        assert!(!r.is_empty());
        let z = 16.5;
        let mut prev = i32::MAX;
        let x0 = WORLD_SIZE_F64 - 40.0 * 32.0;
        for k in 0..(69 * 32) {
            let x = wrap_f64(x0 + 16.5 + f64::from(k));
            match r.at(x, z) {
                Some(RiverHit::Channel { water_top, bed }) => {
                    assert!(water_top <= prev, "rises at x = {x}");
                    // Game gauge, order 1: 2 m deep in the middle.
                    assert!(bed <= water_top - 3, "x = {x}");
                    prev = water_top;
                }
                other => panic!("no channel at x = {x}: {other:?}"),
            }
        }
        // After the pit (cell 30, 5 m) the surface stays at or below 5 m.
        assert!(prev <= 5);
    }

    #[test]
    fn channel_bank_valley_then_nothing() {
        let r = straight(&params(0.0), 1.0, true);
        let x = 500.5;
        assert!(matches!(r.at(x, 16.5), Some(RiverHit::Channel { .. })));
        assert!(matches!(r.at(x, 16.5 + 4.0), Some(RiverHit::Bank { .. })));
        assert!(matches!(
            r.at(x, 16.5 + 10.0),
            Some(RiverHit::Valley { .. })
        ));
        assert_eq!(r.at(x, 16.5 + 40.0), None);
    }

    /// On a gentle slope the trace meanders away from the straight line; on a steep one it
    /// stays close to it.
    #[test]
    fn meanders_on_gentle_slopes_only() {
        let off_axis = |r: &Rivers| {
            (0..2_000)
                .map(|k| {
                    let x = 200.5 + f64::from(k);
                    (-40..=40)
                        .map(|dz| 16.5 + f64::from(dz))
                        .filter(|&z| matches!(r.at(x, z), Some(RiverHit::Channel { .. })))
                        .map(|z| (z - 16.5).abs())
                        .fold(0.0f64, f64::max)
                })
                .fold(0.0f64, f64::max)
        };
        let gentle = straight(&params(75.0), 0.05, false);
        let steep = straight(&params(75.0), 3.0, false);
        assert!(off_axis(&gentle) > 12.0, "{}", off_axis(&gentle));
        assert!(off_axis(&steep) < 6.0, "{}", off_axis(&steep));
    }

    #[test]
    fn frozen_cells_carry_no_water() {
        let res = (WORLD_SIZE_F64 / 32.0) as usize;
        let dir = PeriodicGrid::from_fn(
            res,
            res,
            32.0,
            |i, j| {
                if j == 0 && i < 50 { 0 } else { NO_DIR }
            },
        );
        let width = dir.map(|&d| if d == 0 { 4.0 } else { 0.0 });
        let depth = width.clone();
        let ground = PeriodicGrid::from_fn(res, res, 32.0, |i, _| 100.0 - i as f32);
        let frozen: Vec<bool> = (0..res * res).map(|k| k % res < 20 && k < res).collect();
        let r = Rivers::new(
            &dir,
            &width,
            &depth,
            &ground,
            &ground.map(|_| f32::NEG_INFINITY),
            &frozen,
            &params(0.0),
        );
        assert_eq!(r.at(10.0 * 32.0 + 16.5, 16.5), None);
        assert!(matches!(
            r.at(30.0 * 32.0 + 16.5, 16.5),
            Some(RiverHit::Channel { .. })
        ));
    }
}
