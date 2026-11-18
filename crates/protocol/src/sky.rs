//! `WorldSky`: what the client needs to draw the sky and the far ground, sent once per
//! connection (and kept in the client's disk cache, keyed by the world's fingerprint).
//!
//! - The world's clock, astronomy, lapse rate and atmosphere (no seed: it would let a client
//!   generate the world, geology included).
//! - The reduced climate: on [`CLIMATE_RES`]² cells of 800 m, the annual harmonics of the cloud
//!   cover, snow fraction, surface temperature and humidity, as cartesian components
//!   `(X0, A1·cos, A1·sin, A2·cos, A2·sin)` (they average and quantize without a wrap of the
//!   phase), the daily temperature range and the mean cell height. Each field is quantized on
//!   16 bits over its own range.
//! - The colours of the surface seen from far, [`GROUND_RES`]² RGB.
//!
//! Nothing else: no geology, no relief beyond the mean height of an 800 m cell.

use anyhow::{Context, ensure};
use nascio_core::AtmosphereConfig;
use nascio_core::astro::AstroConfig;
use nascio_core::climate::Harmonics;
use nascio_core::fmath::DetMath;
use nascio_core::time::GameClock;
use serde::{Deserialize, Serialize};

/// Side of the reduced climate grid, cells (800 m each).
pub const CLIMATE_RES: usize = 125;
/// Side of the far ground texture, texels (195 m each).
pub const GROUND_RES: usize = 512;
/// Quantities carried as annual harmonics, in order.
pub const HARMONIC_QUANTITIES: [&str; 4] =
    ["cloud_cover", "snow_fraction", "temperature", "humidity"];
/// Fields of [`ReducedClimate::fields`], in order: five components per harmonic quantity,
/// then the daily range and the mean cell height.
pub const FIELDS: [&str; 22] = [
    "cloud_cover_x0",
    "cloud_cover_c1",
    "cloud_cover_s1",
    "cloud_cover_c2",
    "cloud_cover_s2",
    "snow_fraction_x0",
    "snow_fraction_c1",
    "snow_fraction_s1",
    "snow_fraction_c2",
    "snow_fraction_s2",
    "temperature_x0",
    "temperature_c1",
    "temperature_s1",
    "temperature_c2",
    "temperature_s2",
    "humidity_x0",
    "humidity_c1",
    "humidity_s1",
    "humidity_c2",
    "humidity_s2",
    "diurnal_range",
    "cell_height",
];
/// Largest decompressed `WorldSky` accepted.
pub const MAX_SKY_BYTES: usize = 8 << 20;

const ZSTD_LEVEL: i32 = 9;

/// The world's settings the sky and the HUD read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkyWorld {
    pub clock: GameClock,
    pub astro: AstroConfig,
    /// °C per m (snow line, HUD weather).
    pub lapse_rate_c_per_m: f64,
    pub atmosphere: AtmosphereConfig,
}

/// One field of the reduced climate: `value = min + (max − min) · q / 65 535`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub min: f32,
    pub max: f32,
    /// `CLIMATE_RES²` values, row-major in z, as differences from the previous one (wrapping):
    /// smooth fields compress better.
    pub q: Vec<u16>,
}

impl Field {
    /// Quantizes `values`.
    pub fn new(values: &[f64]) -> Self {
        let min = values.iter().copied().fold(f64::INFINITY, f64::min) as f32;
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max) as f32;
        let span = f64::from(max - min);
        let mut prev = 0u16;
        let q = values
            .iter()
            .map(|&v| {
                let k = if span > 0.0 {
                    ((v - f64::from(min)) / span * 65_535.0)
                        .round()
                        .clamp(0.0, 65_535.0) as u16
                } else {
                    0
                };
                let d = k.wrapping_sub(prev);
                prev = k;
                d
            })
            .collect();
        Self { min, max, q }
    }

    /// The values back.
    pub fn values(&self) -> Vec<f64> {
        let span = f64::from(self.max) - f64::from(self.min);
        let mut k = 0u16;
        self.q
            .iter()
            .map(|&d| {
                k = k.wrapping_add(d);
                f64::from(self.min) + span * f64::from(k) / 65_535.0
            })
            .collect()
    }
}

/// The climate on [`CLIMATE_RES`]² cells: the [`FIELDS`], in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReducedClimate {
    pub fields: Vec<Field>,
}

/// The decoded reduced climate.
pub struct Climate {
    /// Per harmonic quantity ([`HARMONIC_QUANTITIES`]), per cell.
    pub harmonics: Vec<Vec<Harmonics>>,
    pub diurnal_range: Vec<f64>,
    pub cell_height: Vec<f64>,
}

/// Cartesian components of harmonics: `(X0, A1·cos, A1·sin, A2·cos, A2·sin)`, phases taken as
/// angles of the year (the first) and of the half year (the second).
pub fn components(h: &Harmonics) -> [f64; 5] {
    let year = nascio_core::consts::DAYS_PER_YEAR as f64;
    let w1 = std::f64::consts::TAU * h.p1 / year;
    let w2 = 2.0 * std::f64::consts::TAU * h.p2 / year;
    [
        h.x0,
        h.a1 * w1.det_cos(),
        h.a1 * w1.det_sin(),
        h.a2 * w2.det_cos(),
        h.a2 * w2.det_sin(),
    ]
}

/// Harmonics from their cartesian components (inverse of [`components`]).
pub fn from_components(c: [f64; 5]) -> Harmonics {
    let year = nascio_core::consts::DAYS_PER_YEAR as f64;
    let tau = std::f64::consts::TAU;
    Harmonics {
        x0: c[0],
        a1: c[1].det_hypot(c[2]),
        p1: (c[2].det_atan2(c[1]) / tau * year).rem_euclid(year),
        a2: c[3].det_hypot(c[4]),
        p2: (c[4].det_atan2(c[3]) / (2.0 * tau) * year).rem_euclid(year / 2.0),
    }
}

impl ReducedClimate {
    pub fn decode(&self) -> anyhow::Result<Climate> {
        let n = CLIMATE_RES * CLIMATE_RES;
        ensure!(
            self.fields.len() == FIELDS.len(),
            "{} climate fields",
            self.fields.len()
        );
        ensure!(
            self.fields.iter().all(|f| f.q.len() == n),
            "climate field of the wrong size"
        );
        let values: Vec<Vec<f64>> = self.fields.iter().map(Field::values).collect();
        let harmonics = (0..HARMONIC_QUANTITIES.len())
            .map(|q| {
                let f = &values[5 * q..5 * q + 5];
                (0..n)
                    .map(|k| from_components([f[0][k], f[1][k], f[2][k], f[3][k], f[4][k]]))
                    .collect()
            })
            .collect();
        Ok(Climate {
            harmonics,
            diurnal_range: values[20].clone(),
            cell_height: values[21].clone(),
        })
    }
}

/// The far ground's colours, `GROUND_RES²` sRGB triplets, row-major in z.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundColors {
    pub rgb: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldSky {
    pub world: SkyWorld,
    pub climate: ReducedClimate,
    pub ground: GroundColors,
}

impl WorldSky {
    /// postcard, then zstd.
    pub fn compress(&self) -> anyhow::Result<Vec<u8>> {
        let raw = postcard::to_stdvec(self)?;
        Ok(zstd::bulk::compress(&raw, ZSTD_LEVEL)?)
    }

    /// Decompresses and checks the sizes.
    pub fn decompress(bytes: &[u8]) -> anyhow::Result<Self> {
        let raw = zstd::bulk::decompress(bytes, MAX_SKY_BYTES).context("world sky")?;
        let sky: Self = postcard::from_bytes(&raw).context("world sky")?;
        sky.climate.decode()?;
        ensure!(
            sky.ground.rgb.len() == GROUND_RES * GROUND_RES * 3,
            "far ground of {} bytes",
            sky.ground.rgb.len()
        );
        Ok(sky)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harmonics_survive_their_components() {
        let h = Harmonics {
            x0: 12.0,
            a1: 8.5,
            p1: 200.0,
            a2: 1.25,
            p2: 30.0,
        };
        let back = from_components(components(&h));
        for day in [0.0, 91.0, 200.0, 364.9] {
            assert!((h.eval(day) - back.eval(day)).abs() < 1e-9);
        }
    }

    #[test]
    fn fields_quantize_on_16_bits() {
        let v: Vec<f64> = (0..1000)
            .map(|k| (k as f64 * 0.01).sin() * 30.0 + 5.0)
            .collect();
        let back = Field::new(&v).values();
        let step = 60.0 / 65_535.0;
        assert!(v.iter().zip(&back).all(|(a, b)| (a - b).abs() <= step));
        // A constant field.
        assert_eq!(Field::new(&[3.0; 4]).values(), vec![3.0; 4]);
    }
}
