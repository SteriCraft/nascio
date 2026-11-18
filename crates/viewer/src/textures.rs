//! Block texture loader: reads `<name>.png` images of a folder into the layers of one GPU
//! texture array (with linear-light mips) and builds the per-face table of `Blocks`.

use std::path::Path;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::log::{info, warn};
use bevy::prelude::Image;
use bevy::render::render_resource::{
    Extent3d, TextureDataOrder, TextureDimension, TextureFormat, TextureViewDescriptor,
    TextureViewDimension,
};
use nascio_chunk::block::{BlockTextures, Blocks, FaceTexture, FaceTint, srgb_to_linear};

/// Edge of a block texture, pixels.
pub const TEXTURE_SIZE: u32 = 32;
/// Mip levels of the array: 32, 16, 8, 4, 2, 1.
pub const MIP_LEVELS: u32 = 6;
/// Default folder of the block textures (local test images, never committed).
#[allow(dead_code)]
pub const DEFAULT_DIR: &str = "textures_temp/blocks";
/// Colour of the spruce leaves (sRGB), applied as a fixed tint.
pub const SPRUCE_GREEN: [u8; 3] = [56, 92, 50];

/// Block name -> file stem, when they differ.
const ALIASES: [(&str, &str); 1] = [("snow_layer", "snow")];
/// Block name -> file stem of the bottom face.
const BOTTOM: [(&str, &str); 2] = [("grass", "dirt"), ("podzol", "dirt")];

/// Block textures loaded from a folder: the layers of the texture array (with their mips) and the
/// per-block face table for `Blocks::set_textures`.
#[derive(Debug)]
#[allow(dead_code)]
pub struct BlockTextureSet {
    /// Per layer, every mip level from 32×32 down to 1×1, sRGB RGBA8, concatenated.
    pub layers: Vec<Vec<u8>>,
    /// File stem of each layer (e.g. "grass_top"), for logs and tests.
    pub names: Vec<String>,
    /// One entry per block id.
    pub table: Vec<BlockTextures>,
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn to_linear_u8(v: u8) -> f32 {
    srgb_to_linear(f32::from(v) / 255.0)
}

fn to_srgb_u8(v: f32) -> u8 {
    (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8
}

/// Reads a 32×32 image; `None` (with a warning when the file exists) if unusable.
fn read_image(path: &Path) -> Option<image::RgbaImage> {
    if !path.is_file() {
        return None;
    }
    match image::open(path) {
        Ok(img) => {
            let img = img.to_rgba8();
            if img.width() == TEXTURE_SIZE && img.height() == TEXTURE_SIZE {
                Some(img)
            } else {
                warn!(
                    "texture {}: {}x{} instead of {TEXTURE_SIZE}x{TEXTURE_SIZE}, ignored",
                    path.display(),
                    img.width(),
                    img.height()
                );
                None
            }
        }
        Err(e) => {
            warn!("texture {}: {e}, ignored", path.display());
            None
        }
    }
}

/// Opaque RGBA8 level 0 (transparent pixels filled with the mean colour), then all mips.
fn build_layer(img: &image::RgbaImage) -> Vec<u8> {
    let mut sum = [0.0f64; 3];
    let mut weight = 0.0f64;
    for p in img.pixels() {
        let a = f64::from(p[3]) / 255.0;
        for c in 0..3 {
            sum[c] += f64::from(p[c]) * a;
        }
        weight += a;
    }
    let mean = if weight > 0.0 {
        [sum[0] / weight, sum[1] / weight, sum[2] / weight]
    } else {
        [0.0; 3]
    };
    let size = TEXTURE_SIZE as usize;
    let mut level: Vec<u8> = Vec::with_capacity(size * size * 4);
    for p in img.pixels() {
        let a = f64::from(p[3]) / 255.0;
        for c in 0..3 {
            let v = f64::from(p[c]) * a + mean[c] * (1.0 - a);
            level.push(v.round().clamp(0.0, 255.0) as u8);
        }
        level.push(255);
    }
    let mut out = level.clone();
    let mut edge = size;
    while edge > 1 {
        let next_edge = edge / 2;
        let mut next = Vec::with_capacity(next_edge * next_edge * 4);
        for y in 0..next_edge {
            for x in 0..next_edge {
                for c in 0..3 {
                    let mut acc = 0.0f32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        acc += to_linear_u8(level[((2 * y + dy) * edge + 2 * x + dx) * 4 + c]);
                    }
                    next.push(to_srgb_u8(acc / 4.0));
                }
                next.push(255);
            }
        }
        out.extend_from_slice(&next);
        level = next;
        edge = next_edge;
    }
    out
}

fn srgb_triplet_to_linear(c: [u8; 3]) -> [f32; 3] {
    [to_linear_u8(c[0]), to_linear_u8(c[1]), to_linear_u8(c[2])]
}

/// Loads the textures of `dir` for `blocks`; `None` if the folder is missing or nothing matched.
#[allow(dead_code)]
pub fn load(dir: &Path, blocks: &Blocks) -> Option<BlockTextureSet> {
    if !dir.is_dir() {
        info!(
            "block textures: {} is not a folder, none loaded",
            dir.display()
        );
        return None;
    }
    let water_tint = blocks
        .id("water")
        .map(|id| FaceTint::Fixed(blocks.linear(id)));
    let mut layers: Vec<Vec<u8>> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    // File stem -> layer (None: unusable file, warned once).
    let mut known: Vec<(String, Option<u16>)> = Vec::new();
    let mut table = vec![BlockTextures::default(); blocks.len()];
    let mut textured = 0usize;

    let mut face = |stem: &str| -> Option<FaceTexture> {
        let layer = match known.iter().find(|(s, _)| s == stem) {
            Some((_, l)) => *l,
            None => {
                let layer = read_image(&dir.join(format!("{stem}.png"))).map(|img| {
                    layers.push(build_layer(&img));
                    names.push(stem.to_string());
                    (layers.len() - 1) as u16
                });
                known.push((stem.to_string(), layer));
                layer
            }
        }?;
        let tint = match stem {
            "grass_top" => FaceTint::Climate,
            "spruce_leaves" => FaceTint::Fixed(srgb_triplet_to_linear(SPRUCE_GREEN)),
            "water" => water_tint?,
            _ => FaceTint::None,
        };
        Some(FaceTexture { layer, tint })
    };

    for (id, slot) in table.iter_mut().enumerate() {
        let name = blocks.def(id as u16).name.as_str();
        if name == "air" {
            continue;
        }
        let stem = ALIASES
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(name, |(_, s)| *s);
        let exists = |s: &str| dir.join(format!("{s}.png")).is_file();
        let top_stem = format!("{stem}_top");
        let top_stem = if exists(&top_stem) {
            top_stem
        } else {
            stem.to_string()
        };
        let bottom_stem = match BOTTOM.iter().find(|(n, _)| *n == name) {
            Some((_, s)) => (*s).to_string(),
            None => {
                let b = format!("{stem}_bottom");
                if exists(&b) { b } else { stem.to_string() }
            }
        };
        let t = BlockTextures {
            top: face(&top_stem),
            side: face(stem),
            bottom: face(&bottom_stem),
        };
        if t != BlockTextures::default() {
            textured += 1;
        }
        *slot = t;
    }

    if layers.is_empty() {
        info!(
            "block textures: no usable image in {}, none loaded",
            dir.display()
        );
        return None;
    }
    info!(
        "block textures: {} layers, {textured} textured blocks ({})",
        layers.len(),
        dir.display()
    );
    Some(BlockTextureSet {
        layers,
        names,
        table,
    })
}

/// The texture array as a Bevy image (layer-major data, mips, `D2Array` view).
#[allow(dead_code)]
pub fn to_image(set: &BlockTextureSet) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: TEXTURE_SIZE,
            height: TEXTURE_SIZE,
            depth_or_array_layers: set.layers.len() as u32,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = MIP_LEVELS;
    image.data = Some(set.layers.concat());
    image.data_order = TextureDataOrder::LayerMajor;
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..Default::default()
    });
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Nearest,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..Default::default()
    });
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const LAYER_BYTES: usize = 4 * (32 * 32 + 16 * 16 + 8 * 8 + 4 * 4 + 2 * 2 + 1);

    struct Dir(PathBuf);
    impl Dir {
        fn new(test: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "nascio-textures-{}-{}",
                std::process::id(),
                test
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        fn solid(&self, name: &str, rgba: [u8; 4]) {
            let img = image::RgbaImage::from_pixel(32, 32, image::Rgba(rgba));
            img.save(self.0.join(format!("{name}.png"))).unwrap();
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn layer_of(set: &BlockTextureSet, f: Option<FaceTexture>) -> &str {
        &set.names[usize::from(f.expect("textured face").layer)]
    }

    #[test]
    fn missing_and_empty_folders() {
        let blocks = Blocks::builtin();
        let d = Dir::new("empty");
        assert!(load(&d.0.join("nope"), &blocks).is_none());
        assert!(load(&d.0, &blocks).is_none());
    }

    #[test]
    fn face_rules() {
        let blocks = Blocks::builtin();
        let d = Dir::new("rules");
        for (i, n) in [
            "dirt",
            "grass",
            "grass_top",
            "spruce_log",
            "spruce_log_top",
            "snow",
            "podzol",
        ]
        .iter()
        .enumerate()
        {
            d.solid(n, [10 * i as u8 + 10, 50, 90, 255]);
        }
        let set = load(&d.0, &blocks).unwrap();
        let t = |n: &str| set.table[usize::from(blocks.id(n).unwrap())];

        let g = t("grass");
        assert_eq!(layer_of(&set, g.top), "grass_top");
        assert_eq!(g.top.unwrap().tint, FaceTint::Climate);
        assert_eq!(layer_of(&set, g.side), "grass");
        assert_eq!(g.side.unwrap().tint, FaceTint::None);
        assert_eq!(layer_of(&set, g.bottom), "dirt");
        let dirt = t("dirt");
        assert_eq!(g.bottom, dirt.side);
        assert_eq!(dirt.top, dirt.side);
        assert_eq!(dirt.bottom, dirt.side);

        assert_eq!(t("podzol").bottom, dirt.side);
        assert_eq!(layer_of(&set, t("podzol").top), "podzol");

        let log = t("spruce_log");
        assert_eq!(layer_of(&set, log.top), "spruce_log_top");
        assert_eq!(layer_of(&set, log.side), "spruce_log");
        assert_eq!(layer_of(&set, log.bottom), "spruce_log");

        let snow = t("snow_layer");
        assert_eq!(layer_of(&set, snow.top), "snow");
        assert_eq!(snow.top, snow.side);
        assert_eq!(snow.top, snow.bottom);

        assert_eq!(t("granite"), BlockTextures::default());
        assert_eq!(t("air"), BlockTextures::default());
        assert_eq!(set.layers.len(), 7);
        assert_eq!(set.names.len(), 7);
        assert_eq!(set.table.len(), blocks.len());
    }

    #[test]
    fn wrong_size_is_skipped() {
        let blocks = Blocks::builtin();
        let d = Dir::new("size");
        image::RgbaImage::new(16, 16)
            .save(d.0.join("granite.png"))
            .unwrap();
        d.solid("dirt", [1, 2, 3, 255]);
        let set = load(&d.0, &blocks).unwrap();
        assert_eq!(
            set.table[usize::from(blocks.id("granite").unwrap())],
            BlockTextures::default()
        );
        assert_eq!(set.layers.len(), 1);
    }

    #[test]
    fn tints() {
        let blocks = Blocks::builtin();
        let d = Dir::new("tints");
        d.solid("spruce_leaves", [40, 80, 40, 255]);
        d.solid("water", [30, 60, 120, 255]);
        let set = load(&d.0, &blocks).unwrap();
        let leaves = set.table[usize::from(blocks.id("spruce_leaves").unwrap())];
        assert_eq!(
            leaves.top.unwrap().tint,
            FaceTint::Fixed(srgb_triplet_to_linear(SPRUCE_GREEN))
        );
        let wid = blocks.id("water").unwrap();
        assert_eq!(
            set.table[usize::from(wid)].side.unwrap().tint,
            FaceTint::Fixed(blocks.linear(wid))
        );
    }

    #[test]
    fn pixels_and_mips() {
        let blocks = Blocks::builtin();
        let d = Dir::new("pixels");
        d.solid("dirt", [120, 80, 40, 255]);
        d.solid("granite", [200, 100, 50, 128]);
        let mut checker = image::RgbaImage::new(32, 32);
        for (x, y, p) in checker.enumerate_pixels_mut() {
            let v = if (x + y) % 2 == 0 { 0 } else { 255 };
            *p = image::Rgba([v, v, v, 255]);
        }
        checker.save(d.0.join("podzol.png")).unwrap();
        let set = load(&d.0, &blocks).unwrap();
        for l in &set.layers {
            assert_eq!(l.len(), LAYER_BYTES);
        }
        let idx = |n: &str| set.names.iter().position(|x| x == n).unwrap();

        for px in set.layers[idx("dirt")].chunks(4) {
            assert!(px[0].abs_diff(120) <= 1 && px[1].abs_diff(80) <= 1 && px[2].abs_diff(40) <= 1);
            assert_eq!(px[3], 255);
        }
        for px in set.layers[idx("granite")].chunks(4) {
            assert_eq!(px[3], 255);
            assert!(px[0].abs_diff(200) <= 1);
        }
        let last = &set.layers[idx("podzol")][LAYER_BYTES - 4..];
        assert!((186..=190).contains(&last[0]), "got {}", last[0]);
    }

    #[test]
    fn deterministic() {
        let blocks = Blocks::builtin();
        let d = Dir::new("det");
        d.solid("dirt", [1, 2, 3, 255]);
        d.solid("grass_top", [4, 5, 6, 255]);
        d.solid("grass", [7, 8, 9, 255]);
        let a = load(&d.0, &blocks).unwrap();
        let b = load(&d.0, &blocks).unwrap();
        assert_eq!(a.layers, b.layers);
        assert_eq!(a.names, b.names);
        assert_eq!(a.table, b.table);
    }

    #[test]
    fn image_layout() {
        let blocks = Blocks::builtin();
        let d = Dir::new("image");
        d.solid("dirt", [1, 2, 3, 255]);
        let set = load(&d.0, &blocks).unwrap();
        let img = to_image(&set);
        assert_eq!(img.texture_descriptor.size.depth_or_array_layers, 1);
        assert_eq!(img.texture_descriptor.mip_level_count, MIP_LEVELS);
        assert_eq!(
            img.texture_view_descriptor.unwrap().dimension,
            Some(TextureViewDimension::D2Array)
        );
        assert_eq!(img.data.unwrap().len(), LAYER_BYTES);
    }
}
