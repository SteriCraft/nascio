//! Codec of the horizon tiles ([`nascio_chunk::horizon`]), version [`TILE_FORMAT`].
//!
//! Planar layout, then zstd: key (level, i, j as zigzag varints), heights in 1/16 m as varint
//! differences from the previous vertex, water levels likewise, one surface byte per vertex,
//! then the colour of each ground or water vertex (the bottom's, under water) as byte
//! differences from the previous one; ice vertices carry no colour.

use anyhow::{Context, bail, ensure};
use nascio_chunk::horizon::{
    HEIGHT_STEPS, HorizonTile, HorizonVertex, MAX_LEVELS, Surface, TILE, TileKey,
};

use crate::varint::{Reader, put_i};

/// Version of the tile encoding (part of the client's cache paths).
pub const TILE_FORMAT: u32 = 2;
/// Largest compressed or decompressed tile accepted.
pub const MAX_TILE_BYTES: usize = 64 << 10;
/// Tiles a client may ask in one message.
pub const MAX_TILES_PER_REQUEST: usize = 64;

const ZSTD_LEVEL: i32 = 3;

/// Encodes and compresses a tile.
pub fn encode_tile(t: &HorizonTile) -> anyhow::Result<Vec<u8>> {
    ensure!(
        t.vertices.len() == TILE * TILE,
        "tile of {} vertices",
        t.vertices.len()
    );
    let mut out = Vec::with_capacity(TILE * TILE * 4);
    out.push(t.key.level);
    put_i(&mut out, i64::from(t.key.i));
    put_i(&mut out, i64::from(t.key.j));
    let mut prev = 0i64;
    for v in &t.vertices {
        let q = (f64::from(v.height) * HEIGHT_STEPS).round() as i64;
        put_i(&mut out, q - prev);
        prev = q;
    }
    let mut prev = 0i64;
    for v in &t.vertices {
        let w = v.water.round() as i64;
        put_i(&mut out, w - prev);
        prev = w;
    }
    out.extend(t.vertices.iter().map(|v| v.surface as u8));
    let mut prev = [0u8; 3];
    for v in t.vertices.iter().filter(|v| has_color(v.surface)) {
        out.extend((0..3).map(|c| v.color[c].wrapping_sub(prev[c])));
        prev = v.color;
    }
    Ok(zstd::bulk::compress(&out, ZSTD_LEVEL)?)
}

/// Whether a vertex of this surface carries a colour (ice takes the block table's).
fn has_color(s: Surface) -> bool {
    s != Surface::Ice
}

/// Decompresses and decodes a tile, checking every field.
pub fn decode_tile(bytes: &[u8]) -> anyhow::Result<HorizonTile> {
    let raw = zstd::bulk::decompress(bytes, MAX_TILE_BYTES).context("horizon tile")?;
    let mut r = Reader::new(&raw);
    let level = r.u8()?;
    ensure!(level < MAX_LEVELS, "horizon level {level}");
    let coord =
        |r: &mut Reader| -> anyhow::Result<i32> { i32::try_from(r.var_i()?).context("tile index") };
    let key = TileKey {
        level,
        i: coord(&mut r)?,
        j: coord(&mut r)?,
    };
    let n = TILE * TILE;
    let mut heights = Vec::with_capacity(n);
    let mut q = 0i64;
    for _ in 0..n {
        q = q.checked_add(r.var_i()?).context("height")?;
        ensure!(q.abs() < 1 << 24, "height {q}");
        heights.push((q as f64 / HEIGHT_STEPS) as f32);
    }
    let mut waters = Vec::with_capacity(n);
    let mut w = 0i64;
    for _ in 0..n {
        w = w.checked_add(r.var_i()?).context("water")?;
        ensure!(w.abs() < 1 << 20, "water {w}");
        waters.push(w as f32);
    }
    let surfaces: Vec<Surface> = r
        .take(n)?
        .iter()
        .map(|&b| Surface::from_u8(b).with_context(|| format!("surface {b}")))
        .collect::<anyhow::Result<_>>()?;
    let mut prev = [0u8; 3];
    let mut vertices = Vec::with_capacity(n);
    for k in 0..n {
        let color = if has_color(surfaces[k]) {
            let d = r.take(3)?;
            prev = [0, 1, 2].map(|c| prev[c].wrapping_add(d[c]));
            prev
        } else {
            [0; 3]
        };
        vertices.push(HorizonVertex {
            height: heights[k],
            water: waters[k],
            surface: surfaces[k],
            color,
        });
    }
    if !r.is_empty() {
        bail!("{} bytes after the tile", r.rest().len());
    }
    Ok(HorizonTile { key, vertices })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile() -> HorizonTile {
        let vertices = (0..TILE * TILE)
            .map(|k| {
                let h = (k as f32 * 0.37).sin() * 40.0 - 3.0;
                let height = (f64::from(h) * HEIGHT_STEPS).round() as f32 / HEIGHT_STEPS as f32;
                let water = if k % 7 == 0 { 12.0 } else { 0.0 };
                let surface = match (height < water, k % 3) {
                    (false, _) => Surface::Ground,
                    (true, 0) => Surface::Ice,
                    (true, _) => Surface::Water,
                };
                HorizonVertex {
                    height,
                    water,
                    surface,
                    color: if surface != Surface::Ice {
                        [(k % 256) as u8, 200, (255 - k % 256) as u8]
                    } else {
                        [0; 3]
                    },
                }
            })
            .collect();
        HorizonTile {
            key: TileKey {
                level: 3,
                i: -7,
                j: 195,
            },
            vertices,
        }
    }

    #[test]
    fn tile_round_trip() {
        let t = tile();
        let bytes = encode_tile(&t).expect("encodes");
        assert_eq!(decode_tile(&bytes).expect("decodes"), t);
    }

    #[test]
    fn bad_tiles_are_refused() {
        assert!(decode_tile(b"not zstd").is_err());
        let mut raw = vec![MAX_LEVELS];
        raw.extend([0, 0]);
        let z = zstd::bulk::compress(&raw, 3).expect("compresses");
        assert!(decode_tile(&z).is_err());
        // Truncated.
        let t = encode_tile(&tile()).expect("encodes");
        let raw = zstd::bulk::decompress(&t, MAX_TILE_BYTES).expect("decompresses");
        let z = zstd::bulk::compress(&raw[..raw.len() - 1], 3).expect("compresses");
        assert!(decode_tile(&z).is_err());
    }
}
