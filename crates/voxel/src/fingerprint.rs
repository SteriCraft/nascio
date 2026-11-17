//! Generation identity of a world: its layers and files ([`World::hash`]) plus the `[voxel]`
//! configuration that turns them into chunks (decision of 2026-11-17, audit M7 E4).
//!
//! The chunk configuration is the generator's default, [`VoxelConfig::default`]: changing it
//! changes the chunks, so it bumps `GENERATOR_VERSION` (the test below ties the two). Another
//! configuration is refused, except with an explicit developer override; everything produced
//! under an override is marked as such ([`DEV_OVERRIDE_MARK`]).

use std::path::{Path, PathBuf};

use anyhow::bail;
use nascio_world::world_io::World;

use crate::terrain::VoxelConfig;

/// Mark carried by every output produced with a non-default `[voxel]` config.
pub const DEV_OVERRIDE_MARK: &str = "DEV-OVERRIDE";

impl VoxelConfig {
    /// Canonical TOML (`[voxel]` table), the input of [`VoxelConfig::hash`].
    pub fn to_toml(&self) -> String {
        #[derive(serde::Serialize)]
        struct File<'a> {
            voxel: &'a VoxelConfig,
        }
        toml::to_string(&File { voxel: self }).expect("VoxelConfig serialises")
    }

    /// blake3 of [`VoxelConfig::to_toml`], as hex.
    pub fn hash(&self) -> String {
        blake3::hash(self.to_toml().as_bytes()).to_hex().to_string()
    }
}

/// The `[voxel]` config chunks are generated with, and whether it is a developer override.
#[derive(Clone, Debug)]
pub struct GenConfig {
    pub voxel: VoxelConfig,
    /// The config differs from the generator's default: outputs must be marked.
    pub dev_override: bool,
}

impl GenConfig {
    /// The generator's config.
    pub fn standard() -> Self {
        Self {
            voxel: VoxelConfig::default(),
            dev_override: false,
        }
    }

    /// The generator's config, or the TOML file `path` when `dev_override` allows it.
    pub fn load(path: Option<&Path>, dev_override: bool) -> anyhow::Result<Self> {
        let Some(path) = path else {
            return Ok(Self::standard());
        };
        if !dev_override {
            bail!(
                "a [voxel] config other than the generator's changes the chunks of the world \
                 (its fingerprint); pass --dev-override to use {} anyway",
                path.display()
            );
        }
        let voxel = VoxelConfig::from_toml(&std::fs::read_to_string(path)?)?;
        let dev_override = voxel != VoxelConfig::default();
        if dev_override {
            eprintln!(
                "{DEV_OVERRIDE_MARK}: [voxel] from {} (outputs are marked)",
                path.display()
            );
        }
        Ok(Self {
            voxel,
            dev_override,
        })
    }

    /// Generation fingerprint of `world` with this config (deltas are tied to it).
    pub fn fingerprint(&self, world: &World) -> String {
        fingerprint(&world.hash(), &self.voxel)
    }

    /// `path` with `-dev-override` before its extension under an override, else unchanged.
    pub fn mark_path(&self, path: PathBuf) -> PathBuf {
        if !self.dev_override {
            return path;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = match path.extension() {
            Some(e) => format!("{stem}-dev-override.{}", e.to_string_lossy()),
            None => format!("{stem}-dev-override"),
        };
        path.with_file_name(name)
    }

    /// `"DEV-OVERRIDE "` under an override, else empty (prefix for printed lines and titles).
    pub fn mark(&self) -> &'static str {
        if self.dev_override {
            "DEV-OVERRIDE "
        } else {
            ""
        }
    }
}

/// blake3 of the world hash and of the `[voxel]` config hash, as hex.
pub fn fingerprint(world_hash: &str, voxel: &VoxelConfig) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"nascio-generation\0");
    h.update(world_hash.as_bytes());
    h.update(&[0]);
    h.update(voxel.hash().as_bytes());
    h.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nascio_world::config::GENERATOR_VERSION;

    /// Changing `VoxelConfig::default()` changes the chunks: bump `GENERATOR_VERSION`, then
    /// update this pair.
    #[test]
    fn default_config_is_tied_to_the_generator_version() {
        let got = (GENERATOR_VERSION, VoxelConfig::default().hash());
        assert_eq!(
            got,
            (
                19,
                String::from("a1c646cf6579af67cac165f200589fb3899673fb26dfa9ce565c331ae9825fab")
            ),
            "VoxelConfig::default() changed: bump GENERATOR_VERSION and update this test"
        );
    }

    #[test]
    fn toml_round_trip() {
        let c = VoxelConfig::default();
        assert_eq!(VoxelConfig::from_toml(&c.to_toml()).unwrap(), c);
    }

    #[test]
    fn override_needs_the_flag_and_is_marked() {
        assert!(GenConfig::load(Some(Path::new("x.toml")), false).is_err());
        let std = GenConfig::standard();
        assert_eq!(std.mark_path("a/b.png".into()), PathBuf::from("a/b.png"));
        let dev = GenConfig {
            voxel: VoxelConfig::default(),
            dev_override: true,
        };
        assert_eq!(
            dev.mark_path("a/b.png".into()),
            PathBuf::from("a/b-dev-override.png")
        );
        let mut other = VoxelConfig::default();
        other.biome_dither_m += 1.0;
        assert_ne!(
            fingerprint("w", &other),
            fingerprint("w", &VoxelConfig::default())
        );
    }
}
