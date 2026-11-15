//! On-disk world format (spec §6):
//!
//! ```text
//! world-<seed>/
//!   world.toml          config (seed, generator version, parameters, GAME_EPOCH)
//!   layers/<name>.bin   one layer per file
//!   previews/<name>.png previews
//! ```
//!
//! Layer file, all little-endian:
//!
//! | bytes | field |
//! |---|---|
//! | 4 | magic `NSCL` |
//! | 2 | format version (1) |
//! | 1 | data type: 1 = u8, 2 = u16, 3 = f32 |
//! | 1 | reserved (0) |
//! | 4 | width |
//! | 4 | height |
//! | 8 | cell size in metres (f64) |
//! | 2 | unit length `n` |
//! | n | unit, UTF-8 |
//! | … | `width × height` values, row-major (z rows of x cells) |

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};

use crate::config::WorldConfig;
use crate::grid::PeriodicGrid;

const MAGIC: &[u8; 4] = b"NSCL";
const FORMAT_VERSION: u16 = 1;

/// Values of a layer.
#[derive(Clone, Debug, PartialEq)]
pub enum LayerData {
    U8(Vec<u8>),
    U16(Vec<u16>),
    F32(Vec<f32>),
}

impl LayerData {
    fn type_code(&self) -> u8 {
        match self {
            LayerData::U8(_) => 1,
            LayerData::U16(_) => 2,
            LayerData::F32(_) => 3,
        }
    }

    fn len(&self) -> usize {
        match self {
            LayerData::U8(v) => v.len(),
            LayerData::U16(v) => v.len(),
            LayerData::F32(v) => v.len(),
        }
    }

    /// Value at an index, as `f64`.
    pub fn get_f64(&self, i: usize) -> f64 {
        match self {
            LayerData::U8(v) => f64::from(v[i]),
            LayerData::U16(v) => f64::from(v[i]),
            LayerData::F32(v) => f64::from(v[i]),
        }
    }

    fn write_le(&self, out: &mut Vec<u8>) {
        match self {
            LayerData::U8(v) => out.extend_from_slice(v),
            LayerData::U16(v) => v
                .iter()
                .for_each(|x| out.extend_from_slice(&x.to_le_bytes())),
            LayerData::F32(v) => v
                .iter()
                .for_each(|x| out.extend_from_slice(&x.to_le_bytes())),
        }
    }
}

/// A named 2D layer covering the torus.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    pub unit: String,
    pub width: usize,
    pub height: usize,
    pub cell_m: f64,
    pub data: LayerData,
}

impl Layer {
    /// Layer from a `u8` grid.
    pub fn from_u8(name: &str, unit: &str, g: &PeriodicGrid<u8>) -> Self {
        Self::new(name, unit, g, LayerData::U8(g.data().to_vec()))
    }

    /// Layer from a `u16` grid.
    pub fn from_u16(name: &str, unit: &str, g: &PeriodicGrid<u16>) -> Self {
        Self::new(name, unit, g, LayerData::U16(g.data().to_vec()))
    }

    /// Layer from an `f32` grid.
    pub fn from_f32(name: &str, unit: &str, g: &PeriodicGrid<f32>) -> Self {
        Self::new(name, unit, g, LayerData::F32(g.data().to_vec()))
    }

    fn new<T>(name: &str, unit: &str, g: &PeriodicGrid<T>, data: LayerData) -> Self {
        Self {
            name: name.into(),
            unit: unit.into(),
            width: g.width(),
            height: g.height(),
            cell_m: g.cell_size(),
            data,
        }
    }

    /// Serialised file contents.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.unit.len() + self.data.len() * 4);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.push(self.data.type_code());
        out.push(0);
        out.extend_from_slice(&(self.width as u32).to_le_bytes());
        out.extend_from_slice(&(self.height as u32).to_le_bytes());
        out.extend_from_slice(&self.cell_m.to_le_bytes());
        out.extend_from_slice(&(self.unit.len() as u16).to_le_bytes());
        out.extend_from_slice(self.unit.as_bytes());
        self.data.write_le(&mut out);
        out
    }

    /// Parses file contents; `name` is taken from the file name.
    pub fn from_bytes(name: &str, bytes: &[u8]) -> anyhow::Result<Self> {
        let mut r = bytes;
        let mut take = |n: usize| -> anyhow::Result<&[u8]> {
            ensure!(r.len() >= n, "truncated layer file");
            let (a, b) = r.split_at(n);
            r = b;
            Ok(a)
        };
        ensure!(take(4)? == MAGIC, "not a Nascio layer file");
        let version = u16::from_le_bytes(take(2)?.try_into()?);
        ensure!(
            version == FORMAT_VERSION,
            "unsupported layer format {version}"
        );
        let ty = take(2)?[0];
        let width = u32::from_le_bytes(take(4)?.try_into()?) as usize;
        let height = u32::from_le_bytes(take(4)?.try_into()?) as usize;
        let cell_m = f64::from_le_bytes(take(8)?.try_into()?);
        let unit_len = u16::from_le_bytes(take(2)?.try_into()?) as usize;
        let unit = String::from_utf8(take(unit_len)?.to_vec())?;
        let n = width * height;
        let data = match ty {
            1 => LayerData::U8(take(n)?.to_vec()),
            2 => LayerData::U16(
                take(n * 2)?
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect(),
            ),
            3 => LayerData::F32(
                take(n * 4)?
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .collect(),
            ),
            t => bail!("unknown layer data type {t}"),
        };
        ensure!(r.is_empty(), "trailing bytes in layer file");
        Ok(Self {
            name: name.into(),
            unit,
            width,
            height,
            cell_m,
            data,
        })
    }

    /// Value at cell `(i, j)` as `f64`.
    pub fn value(&self, i: usize, j: usize) -> f64 {
        self.data.get_f64(j * self.width + i)
    }
}

/// A generated world: its config and its layers.
#[derive(Clone, Debug, PartialEq)]
pub struct World {
    pub config: WorldConfig,
    pub layers: Vec<Layer>,
}

impl World {
    /// Layer by name.
    pub fn layer(&self, name: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.name == name)
    }

    /// Writes `world.toml` and `layers/*.bin` under `dir`.
    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        let layers_dir = dir.join("layers");
        fs::create_dir_all(&layers_dir)
            .with_context(|| format!("creating {}", layers_dir.display()))?;
        fs::write(dir.join("world.toml"), self.config.to_toml()?)?;
        for l in &self.layers {
            let mut f = fs::File::create(layer_path(dir, &l.name))?;
            f.write_all(&l.to_bytes())?;
        }
        Ok(())
    }

    /// Reads `world.toml` and every `layers/*.bin`, sorted by name.
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let toml = fs::read_to_string(dir.join("world.toml"))
            .with_context(|| format!("reading {}", dir.join("world.toml").display()))?;
        let config = WorldConfig::from_toml(&toml)?;
        let mut paths: Vec<PathBuf> = fs::read_dir(dir.join("layers"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "bin"))
            .collect();
        paths.sort();
        let mut layers = Vec::new();
        for p in paths {
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .context("bad layer file name")?
                .to_string();
            let mut bytes = Vec::new();
            fs::File::open(&p)?.read_to_end(&mut bytes)?;
            layers.push(Layer::from_bytes(&name, &bytes)?);
        }
        Ok(Self { config, layers })
    }

    /// blake3 of every layer (name and serialised bytes, in name order), as hex.
    pub fn hash(&self) -> String {
        let mut sorted: Vec<&Layer> = self.layers.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        let mut h = blake3::Hasher::new();
        for l in sorted {
            h.update(l.name.as_bytes());
            h.update(&[0]);
            h.update(&l.to_bytes());
        }
        h.finalize().to_hex().to_string()
    }
}

/// `dir/layers/<name>.bin`.
pub fn layer_path(dir: &Path, name: &str) -> PathBuf {
    dir.join("layers").join(format!("{name}.bin"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Scale;

    fn sample_layers() -> Vec<Layer> {
        let g8 = PeriodicGrid::from_fn(25, 25, 4_000.0, |i, j| (i * 3 + j) as u8);
        let g16 = PeriodicGrid::from_fn(25, 25, 4_000.0, |i, j| (i * 1_000 + j) as u16);
        let gf = PeriodicGrid::from_fn(25, 25, 4_000.0, |i, j| i as f32 * -12.5 + j as f32);
        vec![
            Layer::from_u8("crust_type", "", &g8),
            Layer::from_u16("plate_id", "id", &g16),
            Layer::from_f32("elevation_tectonic", "m", &gf),
        ]
    }

    #[test]
    fn layer_roundtrip_all_types() {
        for l in sample_layers() {
            let back = Layer::from_bytes(&l.name, &l.to_bytes()).unwrap();
            assert_eq!(back, l);
        }
    }

    #[test]
    fn rejects_corrupt_files() {
        let l = &sample_layers()[2];
        let mut b = l.to_bytes();
        assert!(Layer::from_bytes("x", &b[..b.len() - 1]).is_err());
        b[0] = b'X';
        assert!(Layer::from_bytes("x", &b).is_err());
    }

    #[test]
    fn world_save_load_and_hash() {
        let dir = std::env::temp_dir().join(format!("nascio-io-test-{}", std::process::id()));
        let w = World {
            config: WorldConfig::new(3, Scale::TwentyFifth),
            layers: sample_layers(),
        };
        w.save(&dir).unwrap();
        let back = World::load(&dir).unwrap();
        fs::remove_dir_all(&dir).ok();
        assert_eq!(back.config, w.config);
        assert_eq!(back.hash(), w.hash());
        assert_eq!(back.layer("plate_id").unwrap().value(2, 3), 2_003.0);
        // The hash changes with any value.
        let mut other = w.clone();
        if let LayerData::F32(v) = &mut other.layers[2].data {
            v[100] += 1.0;
        }
        assert_ne!(other.hash(), w.hash());
    }
}
