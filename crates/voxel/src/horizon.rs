//! What the horizon and the far ground show, computed from the terrain (spec §7): horizon tiles
//! ([`nascio_chunk::horizon`]) and the colours of the whole surface seen from far.
//!
//! Only the drawn surface comes out: heights, water levels, the ground's colour. The server
//! sends them to clients, the development viewer computes them locally with the same code.

use nascio_chunk::horizon::{
    HorizonTile, HorizonVertex, Surface, TILE, TileKey, linear_to_srgb8, quantize_height, spacing,
    water_color,
};
use nascio_core::consts::WORLD_SIZE_F64;
use nascio_core::fmath::DetMath;
use nascio_core::torus::{wrap, wrap_f64};
use rayon::prelude::*;

use crate::Terrain;

/// Computes a horizon tile.
pub fn horizon_tile(terrain: &Terrain, key: TileKey) -> HorizonTile {
    let s = spacing(key.level);
    let (fi, fj) = key.first_vertex();
    // Heights with a one-vertex margin, for the slopes.
    let side = TILE + 2;
    let pos = |a: usize, b: usize| {
        (
            (fi + a as i64 - 1) as f64 * s,
            (fj + b as i64 - 1) as f64 * s,
        )
    };
    let mut h = Vec::with_capacity(side * side);
    for b in 0..side {
        for a in 0..side {
            let (x, z) = pos(a, b);
            // Block tops sit on average half a metre above the continuous surface.
            h.push(quantize_height(
                terrain.height_lod(wrap_f64(x), wrap_f64(z), 0.5 * s) + 0.5,
            ));
        }
    }
    let at = |a: usize, b: usize| h[a + side * b];
    let beach = terrain.config().beach_high_m + 1;
    let mut vertices = Vec::with_capacity(TILE * TILE);
    for b in 1..=TILE {
        for a in 1..=TILE {
            let (x, z) = pos(a, b);
            let (x, z) = (wrap_f64(x), wrap_f64(z));
            let raw = at(a, b);
            // Sea and lakes only: the coarse surface has no river channels.
            let water = terrain.water_level(x, z).round() as f32;
            let dx = f64::from(at(a + 1, b) - at(a - 1, b)) / (2.0 * s);
            let dz = f64::from(at(a, b + 1) - at(a, b - 1)) / (2.0 * s);
            let (surface, color) = if raw < water {
                if terrain.water_frozen(&terrain.climate(x, z)) {
                    (Surface::Ice, [0; 3])
                } else {
                    // The bottom's colour: the client lays the water over it by depth.
                    let bed = terrain.top_block(
                        wrap(x.floor() as i32),
                        wrap(z.floor() as i32),
                        raw.floor() as i32,
                        dx.det_hypot(dz),
                    );
                    let c = terrain.surface_color(bed, x, z, f64::from(raw));
                    (Surface::Water, c.map(linear_to_srgb8))
                }
            } else {
                // Beaches are too thin to show at horizon scale.
                let top = (raw.floor() as i32).max(beach);
                let c = terrain.far_ground(x, z, top, f64::from(raw), dx.det_hypot(dz));
                (Surface::Ground, c.map(linear_to_srgb8))
            };
            vertices.push(HorizonVertex {
                height: raw,
                water,
                surface,
                color,
            });
        }
    }
    HorizonTile { key, vertices }
}

/// Colours of the world's surface on a `res × res` periodic grid, as seen from far: the
/// horizon's colours (top block, water darkened with depth), RGB8 sRGB, row-major in `z`.
pub fn ground_colors(terrain: &Terrain, res: usize) -> Vec<u8> {
    let cell = WORLD_SIZE_F64 / res as f64;
    let blocks = terrain.blocks();
    let water = blocks.linear(blocks.ids.water);
    (0..res)
        .into_par_iter()
        .flat_map_iter(|j| {
            let mut row = Vec::with_capacity(res * 3);
            for i in 0..res {
                let x = (i as f64 + 0.5) * cell;
                let z = (j as f64 + 0.5) * cell;
                let h = terrain.height_lod(x, z, cell);
                let wl = terrain.water_level(x, z);
                let frozen = h < wl && terrain.water_frozen(&terrain.climate(x, z));
                let dx =
                    terrain.height_lod(x + cell, z, cell) - terrain.height_lod(x - cell, z, cell);
                let dz =
                    terrain.height_lod(x, z + cell, cell) - terrain.height_lod(x, z - cell, cell);
                let c = if frozen {
                    blocks.linear(blocks.ids.ice.unwrap_or(blocks.ids.water))
                } else if h < wl {
                    let bed = terrain.top_block(
                        wrap(x.floor() as i32),
                        wrap(z.floor() as i32),
                        h.floor() as i32,
                        dx.det_hypot(dz) / (2.0 * cell),
                    );
                    water_color(water, terrain.surface_color(bed, x, z, h), wl - h)
                } else {
                    let top = (h.floor() as i32).max(terrain.config().beach_high_m + 1);
                    terrain.far_ground(x, z, top, h, dx.det_hypot(dz) / (2.0 * cell))
                };
                row.extend(c.map(linear_to_srgb8));
            }
            row
        })
        .collect()
}
