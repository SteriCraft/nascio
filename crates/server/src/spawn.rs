//! Spawn sites and the spawn point of each account (chantier 3).
//!
//! The world has `spawn_sites` sites (10), drawn from its seed the first time a server runs it
//! and kept in `<data>/server/spawn_sites.toml`: they stay where they are if the rules change
//! later. At its first login, an account gets a point drawn at random within `spawn_radius_m`
//! (300 m) of a site drawn at random, kept in `accounts.toml`: the player always comes back
//! there.
//!
//! A site, and a player's point, is a block column on dry land (no sea, lake or river water
//! above its top block), lower than `spawn_max_elevation_m` (1 000 m) and outside the polar
//! band (`s > s_clear`, where the polar cloud deck ends, spec §2.7).

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use nascio_core::consts::WORLD_SIZE;
use nascio_core::torus;
use nascio_voxel::Terrain;
use nascio_voxel::delta::write_durable;
use nascio_world::rng::stage_rng;
use nascio_world::world_io::World;
use rand::{Rng, RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use crate::ServerConfig;
use crate::accounts::Accounts;

/// Candidates drawn for the sites before giving up (a world with almost no land).
const SITE_TRIES: usize = 1_000_000;
/// Candidates drawn around a site for a player's point; past them, the site itself.
const POINT_TRIES: usize = 256;

/// The rules of a spawn column.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// The top block stays under this height (m).
    pub max_elevation_m: f64,
    /// Distance to the seam (`s`, m) above which a column leaves the polar band.
    pub polar_s_m: f64,
}

impl Rules {
    /// The top block of column `(x, z)` if it may be a spawn column.
    pub fn top(&self, terrain: &Terrain, x: i32, z: i32) -> Option<i32> {
        let (x, z) = (torus::wrap(x), torus::wrap(z));
        let s = f64::from(z.min(WORLD_SIZE - z));
        if s <= self.polar_s_m {
            return None;
        }
        // Column centres, as the chunk generation samples them.
        let (cx, cz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
        let top = terrain.height(cx, cz).floor() as i32;
        // Water fills `top < y < water_top`: none above a dry column.
        let dry = terrain.water_top(cx, cz) <= top + 1;
        (dry && f64::from(top) < self.max_elevation_m).then_some(top)
    }
}

/// A spawn point: a block column; the player stands on its top block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub z: i32,
}

/// The spawn point of an account, and the site it was drawn around.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSpawn {
    pub site: u32,
    pub x: i32,
    pub z: i32,
}

#[derive(Serialize, Deserialize)]
struct SitesFile {
    /// Fingerprint of the world and generation the sites were drawn for.
    fingerprint: String,
    #[serde(rename = "site")]
    sites: Vec<Point>,
}

/// Draws `n` sites from the world seed (deterministic: the same world gives the same sites).
pub fn draw_sites(
    terrain: &Terrain,
    seed: u64,
    n: usize,
    rules: Rules,
) -> anyhow::Result<Vec<Point>> {
    let mut rng = stage_rng(seed, "spawn/sites");
    let mut sites = Vec::with_capacity(n);
    for _ in 0..SITE_TRIES {
        if sites.len() == n {
            break;
        }
        let (x, z) = (
            rng.random_range(0..WORLD_SIZE),
            rng.random_range(0..WORLD_SIZE),
        );
        if rules.top(terrain, x, z).is_some() {
            sites.push(Point { x, z });
        }
    }
    if sites.len() < n {
        bail!(
            "only {} spawn sites found in {SITE_TRIES} tries (dry land under {} m outside the polar band)",
            sites.len(),
            rules.max_elevation_m
        );
    }
    Ok(sites)
}

/// The sites of the world: those of `dir/spawn_sites.toml` when it was written for this
/// `fingerprint`, otherwise drawn and written there.
pub fn load_or_draw(
    dir: &Path,
    fingerprint: &str,
    terrain: &Terrain,
    seed: u64,
    n: usize,
    rules: Rules,
) -> anyhow::Result<Vec<Point>> {
    let path = dir.join("spawn_sites.toml");
    if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        let file: SitesFile =
            toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
        if file.fingerprint == fingerprint && !file.sites.is_empty() {
            return Ok(file.sites);
        }
        eprintln!("{}: drawn for another world, drawn again", path.display());
    }
    let sites = draw_sites(terrain, seed, n, rules)?;
    std::fs::create_dir_all(dir)?;
    let file = SitesFile {
        fingerprint: fingerprint.to_owned(),
        sites: sites.clone(),
    };
    write_durable(&path, toml::to_string(&file)?.as_bytes())?;
    Ok(sites)
}

/// A point drawn uniformly within `radius_m` of `site` that follows the rules; the site
/// itself if none is found (a site on a small island).
pub fn draw_point(
    terrain: &Terrain,
    site: Point,
    radius_m: f64,
    rules: Rules,
    rng: &mut impl Rng,
) -> Point {
    for _ in 0..POINT_TRIES {
        // One metre less: rounding to a column never puts the point beyond the radius.
        let r = (radius_m - 1.0).max(0.0) * rng.random::<f64>().sqrt();
        let a = std::f64::consts::TAU * rng.random::<f64>();
        let x = torus::wrap(site.x + (r * a.cos()).round() as i32);
        let z = torus::wrap(site.z + (r * a.sin()).round() as i32);
        if rules.top(terrain, x, z).is_some() {
            return Point { x, z };
        }
    }
    site
}

/// Where a player stands at `p`: the centre of the column, on its top block, or on the
/// water (a fixed point at sea).
pub fn standing(terrain: &Terrain, p: Point) -> [f64; 3] {
    let (x, z) = (f64::from(p.x) + 0.5, f64::from(p.z) + 0.5);
    let top = terrain.height(x, z).floor() as i32;
    [x, f64::from((top + 1).max(terrain.water_top(x, z))), z]
}

/// The spawn points of a running server.
pub struct Spawns {
    terrain: Arc<Terrain>,
    /// The sites of the world (empty with a fixed point).
    pub sites: Vec<Point>,
    /// One point for every player (`spawn_xz`).
    pub fixed: Option<Point>,
    radius_m: f64,
    rules: Rules,
}

impl Spawns {
    /// From the configuration: a fixed point, or the sites of `data_dir/server`.
    pub fn new(
        terrain: Arc<Terrain>,
        world: &World,
        fingerprint: &str,
        cfg: &ServerConfig,
        server_dir: &Path,
    ) -> anyhow::Result<Self> {
        let rules = Rules {
            max_elevation_m: cfg.spawn_max_elevation_m,
            polar_s_m: world.config.astro.s_clear,
        };
        let (fixed, sites) = match cfg.spawn_xz {
            Some([x, z]) => (
                Some(Point {
                    x: torus::wrap_f64(x).floor() as i32,
                    z: torus::wrap_f64(z).floor() as i32,
                }),
                Vec::new(),
            ),
            None => (
                None,
                load_or_draw(
                    server_dir,
                    fingerprint,
                    &terrain,
                    world.config.seed,
                    cfg.spawn_sites,
                    rules,
                )?,
            ),
        };
        Ok(Self {
            terrain,
            sites,
            fixed,
            radius_m: cfg.spawn_radius_m,
            rules,
        })
    }

    /// Where `name` appears: the fixed point, or the account's point, drawn (and kept) at its
    /// first login.
    pub fn of(&self, accounts: &Accounts, name: &str) -> anyhow::Result<[f64; 3]> {
        if let Some(p) = self.fixed {
            return Ok(standing(&self.terrain, p));
        }
        let s = match accounts.spawn(name) {
            Some(s) => s,
            None => {
                let mut seed = [0u8; 32];
                SystemRandom::new()
                    .fill(&mut seed)
                    .map_err(|_| anyhow::anyhow!("no system randomness"))?;
                let mut rng = ChaCha8Rng::from_seed(seed);
                let site = rng.random_range(0..self.sites.len());
                let p = draw_point(
                    &self.terrain,
                    self.sites[site],
                    self.radius_m,
                    self.rules,
                    &mut rng,
                );
                let s = AccountSpawn {
                    site: site as u32,
                    x: p.x,
                    z: p.z,
                };
                accounts.set_spawn(name, s)?;
                s
            }
        };
        Ok(standing(&self.terrain, Point { x: s.x, z: s.z }))
    }
}
