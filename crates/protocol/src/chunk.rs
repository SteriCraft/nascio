//! Chunk codec v2 and column batches (S0 measurements, `docs/reports/serveur-S0.md`).
//!
//! A chunk: version byte `2`, palette (count, then block ids of the session table, varints),
//! then — unless the palette has one entry — runs `(palette index, length − 1)` as varints in
//! block index order (`x`, then `z`, then `y`). A uniform chunk takes 3 to 5 bytes.
//!
//! A column batch: the chunks `cy_min..` of one column, each length-prefixed, compressed
//! with zstd (level 3) as a whole. The server builds it once and sends the same bytes to
//! every client.

use anyhow::{Context, bail, ensure};
use nascio_chunk::block::BlockId;
use nascio_chunk::chunk::VOLUME;
use nascio_chunk::{Chunk, ChunkTint};

use crate::varint::{Reader, put, put_i};

/// Version byte of a chunk encoding.
pub const CHUNK_CODEC: u8 = 2;
/// zstd level of the column batches.
pub const ZSTD_LEVEL: i32 = 3;
/// Most bytes a column batch may expand to (decompression bomb guard). A chunk takes at most
/// about 3 bytes per block; 128 chunks of that is far beyond any real column.
pub const MAX_COLUMN_BYTES: usize = 16 << 20;
/// Most chunks in a column batch.
pub const MAX_COLUMN_CHUNKS: usize = 512;

/// Encodes a chunk (codec v2).
pub fn encode(c: &Chunk, out: &mut Vec<u8>) {
    out.push(CHUNK_CODEC);
    let Some(blocks) = c.as_dense() else {
        let id = c.uniform().expect("a chunk without array is uniform");
        put(out, 1);
        put(out, u64::from(id));
        return;
    };
    // Palette in first-use order; looked up only at run boundaries.
    let mut palette: Vec<BlockId> = Vec::new();
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    let mut last = (BlockId::MAX, 0usize);
    for i in 1..=VOLUME {
        if i == VOLUME || blocks[i] != blocks[start] {
            let b = blocks[start];
            let k = if last.0 == b {
                last.1
            } else {
                let k = palette.iter().position(|&q| q == b).unwrap_or_else(|| {
                    palette.push(b);
                    palette.len() - 1
                });
                last = (b, k);
                k
            };
            runs.push((k, i - start));
            start = i;
        }
    }
    put(out, palette.len() as u64);
    for &b in &palette {
        put(out, u64::from(b));
    }
    if palette.len() > 1 {
        for (k, len) in runs {
            put(out, k as u64);
            put(out, (len - 1) as u64);
        }
    }
}

/// Decodes a chunk; `max_id` bounds the block ids (size of the session table).
pub fn decode(r: &mut Reader, max_id: BlockId) -> anyhow::Result<Chunk> {
    ensure!(r.u8()? == CHUNK_CODEC, "unknown chunk codec");
    let n = r.var_max(VOLUME as u64)? as usize;
    ensure!(n > 0, "empty palette");
    let palette: Vec<BlockId> = (0..n)
        .map(|_| Ok(r.var_max(u64::from(max_id))? as BlockId))
        .collect::<anyhow::Result<_>>()?;
    if n == 1 {
        return Ok(Chunk::filled(palette[0]));
    }
    let mut blocks = Box::new([0 as BlockId; VOLUME]);
    let mut at = 0;
    while at < VOLUME {
        let k = r.var()? as usize;
        let len = r.var_max(VOLUME as u64 - 1)? as usize + 1;
        let id = *palette.get(k).context("palette index out of range")?;
        ensure!(at + len <= VOLUME, "too many blocks");
        blocks[at..at + len].fill(id);
        at += len;
    }
    Ok(Chunk::from_blocks(blocks))
}

/// The chunks of one column, from `cy_min` up, as sent to the clients. Above them there is
/// only air (block 0).
#[derive(Clone, PartialEq)]
pub struct Column {
    pub cx: i32,
    pub cz: i32,
    pub cy_min: i32,
    /// Number of the last journal record of the column's region when the column was read
    /// (0: never edited). Edits numbered above it are not in the chunks. Client cache key with
    /// the position.
    pub version: u64,
    pub chunks: Vec<Chunk>,
    /// Render data of the column (grass tint of its corners, water depth of each local
    /// column), computed by the server from the world's climate: the client has no world.
    pub tint: ChunkTint,
}

impl std::fmt::Debug for Column {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Column({}, {}, cy {}.., v{}, {} chunks)",
            self.cx,
            self.cz,
            self.cy_min,
            self.version,
            self.chunks.len()
        )
    }
}

impl Column {
    /// Batch: header and length-prefixed chunks, compressed with zstd.
    pub fn compress(&self) -> anyhow::Result<Vec<u8>> {
        let mut raw = Vec::with_capacity(1024);
        put(&mut raw, u64::from(self.cx as u32));
        put(&mut raw, u64::from(self.cz as u32));
        put_i(&mut raw, i64::from(self.cy_min));
        put(&mut raw, self.version);
        put(&mut raw, self.tint.grass.map_or(0, |g| u64::from(g) + 1));
        for c in self.tint.corners.iter().flatten() {
            raw.extend_from_slice(&c.to_le_bytes());
        }
        raw.extend_from_slice(&self.tint.water_depth);
        put(&mut raw, self.chunks.len() as u64);
        let mut one = Vec::with_capacity(256);
        for c in &self.chunks {
            one.clear();
            encode(c, &mut one);
            put(&mut raw, one.len() as u64);
            raw.extend_from_slice(&one);
        }
        // One compressor per thread: creating a zstd context costs more than a small column.
        thread_local! {
            static Z: std::cell::RefCell<Option<zstd::bulk::Compressor<'static>>> =
                const { std::cell::RefCell::new(None) };
        }
        Z.with(|z| {
            let mut z = z.borrow_mut();
            if z.is_none() {
                *z = Some(zstd::bulk::Compressor::new(ZSTD_LEVEL)?);
            }
            Ok(z.as_mut().expect("compressor").compress(&raw)?)
        })
    }

    /// Inverse of [`compress`](Self::compress); refuses malformed input, ids above `max_id`
    /// and batches that would expand beyond [`MAX_COLUMN_BYTES`].
    pub fn decompress(bytes: &[u8], max_id: BlockId) -> anyhow::Result<Self> {
        let raw = zstd::bulk::decompress(bytes, MAX_COLUMN_BYTES).context("column batch")?;
        let mut r = Reader::new(&raw);
        let cx = r.var_max(u64::from(u32::MAX))? as u32 as i32;
        let cz = r.var_max(u64::from(u32::MAX))? as u32 as i32;
        let cy_min = i32::try_from(r.var_i()?)?;
        let version = r.var()?;
        let grass = match r.var_max(u64::from(max_id) + 1)? {
            0 => None,
            g => Some((g - 1) as BlockId),
        };
        let mut corners = [[0f32; 3]; 4];
        for c in corners.iter_mut().flatten() {
            *c = f32::from_le_bytes(r.take(4)?.try_into()?);
        }
        let water_depth: [u8; 1024] = r.take(1024)?.try_into()?;
        let tint = ChunkTint {
            grass,
            corners,
            water_depth,
        };
        let n = r.var_max(MAX_COLUMN_CHUNKS as u64)? as usize;
        let mut chunks = Vec::with_capacity(n);
        for _ in 0..n {
            let len = r.var_max(raw.len() as u64)? as usize;
            let mut one = Reader::new(r.take(len)?);
            chunks.push(decode(&mut one, max_id)?);
            if !one.is_empty() {
                bail!("trailing bytes in a chunk");
            }
        }
        ensure!(r.is_empty(), "trailing bytes in a column");
        Ok(Self {
            cx,
            cz,
            cy_min,
            version,
            chunks,
            tint,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nascio_chunk::block::AIR;
    use proptest::prelude::*;

    fn round(c: &Chunk) -> (Chunk, usize) {
        let mut b = Vec::new();
        encode(c, &mut b);
        let mut r = Reader::new(&b);
        let back = decode(&mut r, 400).unwrap();
        assert!(r.is_empty());
        (back, b.len())
    }

    #[test]
    fn uniform_chunks_take_a_few_bytes() {
        for id in [AIR, 5, 300] {
            let (back, n) = round(&Chunk::filled(id));
            assert_eq!(back, Chunk::filled(id));
            assert!(n <= 5, "{n} bytes");
        }
        // A dense chunk of one block decodes as uniform too.
        let mut c = Chunk::filled(7);
        c.set(nascio_core::coords::LocalPos { x: 1, y: 2, z: 3 }, 8);
        c.set(nascio_core::coords::LocalPos { x: 1, y: 2, z: 3 }, 7);
        assert_eq!(round(&c).0.uniform(), Some(7));
    }

    #[test]
    fn malformed_input_is_refused() {
        let mut b = Vec::new();
        let mut blocks = Box::new([AIR; VOLUME]);
        blocks[100] = 3;
        encode(&Chunk::from_blocks(blocks), &mut b);
        for cut in 0..b.len() {
            assert!(
                decode(&mut Reader::new(&b[..cut]), 400).is_err(),
                "cut {cut}"
            );
        }
        assert!(
            decode(&mut Reader::new(&b), 2).is_err(),
            "id above the table"
        );
        let mut bad = b.clone();
        bad[0] = 1;
        assert!(decode(&mut Reader::new(&bad), 400).is_err());
        assert!(Column::decompress(b"not zstd", 400).is_err());
    }

    fn chunk() -> impl Strategy<Value = Chunk> {
        // Runs of a few blocks: realistic, and every boundary case of the run coder.
        prop::collection::vec((0u16..6, 1usize..3000), 1..40).prop_map(|runs| {
            let mut b = Box::new([AIR; VOLUME]);
            let mut at = 0;
            for (id, len) in runs.into_iter().cycle() {
                let end = (at + len).min(VOLUME);
                b[at..end].fill(id);
                at = end;
                if at == VOLUME {
                    break;
                }
            }
            Chunk::from_blocks(b)
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn chunks_round_trip(c in chunk()) {
            let (back, n) = round(&c);
            prop_assert!(back == c);
            prop_assert!(n < 2 * VOLUME);
        }

        #[test]
        fn columns_round_trip_across_the_seam(
            chunks in prop::collection::vec(chunk(), 0..6),
            cx in prop::sample::select(vec![0, 1, 3_124]),
            cz in prop::sample::select(vec![0, 3_124]),
            cy_min in -200i32..200,
            version in any::<u64>(),
        ) {
            let mut tint = ChunkTint::NONE;
            tint.grass = Some(3);
            tint.corners[2] = [0.25, 0.5, 0.125];
            tint.water_depth[5] = 7;
            let col = Column { cx, cz, cy_min, version, chunks, tint };
            let bytes = col.compress().unwrap();
            prop_assert_eq!(Column::decompress(&bytes, 400).unwrap(), col);
        }
    }
}
