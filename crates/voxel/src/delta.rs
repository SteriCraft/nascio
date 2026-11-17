//! Delta storage (spec §5: `Chunk = generate(pos) ⊕ delta(pos)`).
//!
//! A delta holds the modified blocks of a chunk as **absolute values** (the block now there,
//! not a difference from the generated one): a modification stays meaningful when the
//! generator changes. Each chunk keeps its modifications sorted by block index; above a
//! threshold they switch to a palette (block ids packed on a few bits, `KEEP` for unmodified
//! blocks).
//!
//! On disk, chunks are grouped in regions of `REGION_CHUNKS`² columns (all heights), one file
//! per region in `world-<seed>/deltas/`. Each file records the generator version and the hash
//! of the world it was made for, and the names of the blocks it uses (ids may change between
//! registries).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};
use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use nascio_core::coords::{BlockPos, ChunkPos};

use crate::block::{BlockId, Blocks};
use crate::chunk::{Chunk, ChunkDeltas, N, PaddedChunk, VOLUME};

/// Side of a region, in chunk columns (800 m). It divides [`CHUNKS_PER_AXIS`] (3125 = 5⁵),
/// so regions tile the torus.
pub const REGION_CHUNKS: i32 = 25;
const _: () = assert!(CHUNKS_PER_AXIS % REGION_CHUNKS == 0);

/// Default number of modified blocks above which a chunk switches to a palette.
pub const DEFAULT_PALETTE_ABOVE: usize = 2048;

const MAGIC: &[u8; 8] = b"NSCDELTA";
const FORMAT: u32 = 1;
/// Palette entry of the blocks left as generated.
const KEEP: BlockId = BlockId::MAX;

/// Modified blocks of one chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChunkDelta {
    /// `(index, block)`, sorted by index ([`LocalPos::index`]).
    Sparse(Vec<(u16, BlockId)>),
    /// Every block of the chunk as a palette index (`palette[0] = KEEP`).
    Palette(PaletteDelta),
}

/// Palette form: `bits` per block (1, 2, 4, 8 or 16), packed in `u64` words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteDelta {
    palette: Vec<BlockId>,
    bits: u8,
    words: Vec<u64>,
    count: usize,
}

fn bits_for(len: usize) -> u8 {
    match len {
        0..=2 => 1,
        3..=4 => 2,
        5..=16 => 4,
        17..=256 => 8,
        _ => 16,
    }
}

impl PaletteDelta {
    fn new(bits: u8) -> Self {
        Self {
            palette: vec![KEEP],
            bits,
            words: vec![0; VOLUME * bits as usize / 64],
            count: 0,
        }
    }

    fn slot(&self, i: usize) -> usize {
        let per = 64 / self.bits as usize;
        let w = self.words[i / per];
        let shift = (i % per) * self.bits as usize;
        ((w >> shift) & ((1u64 << self.bits) - 1)) as usize
    }

    fn put_slot(&mut self, i: usize, v: usize) {
        let per = 64 / self.bits as usize;
        let shift = (i % per) * self.bits as usize;
        let mask = ((1u64 << self.bits) - 1) << shift;
        let w = &mut self.words[i / per];
        *w = (*w & !mask) | ((v as u64) << shift);
    }

    fn get(&self, i: usize) -> Option<BlockId> {
        match self.palette[self.slot(i)] {
            KEEP => None,
            b => Some(b),
        }
    }

    fn set(&mut self, i: usize, id: BlockId) {
        let p = match self.palette.iter().position(|&b| b == id) {
            Some(p) => p,
            None => {
                self.palette.push(id);
                if bits_for(self.palette.len()) > self.bits {
                    let mut wider = PaletteDelta::new(bits_for(self.palette.len()));
                    for k in 0..VOLUME {
                        wider.put_slot(k, self.slot(k));
                    }
                    wider.palette = std::mem::take(&mut self.palette);
                    wider.count = self.count;
                    *self = wider;
                }
                self.palette.len() - 1
            }
        };
        if self.slot(i) == 0 {
            self.count += 1;
        }
        self.put_slot(i, p);
    }
}

impl ChunkDelta {
    /// Number of modified blocks.
    pub fn len(&self) -> usize {
        match self {
            ChunkDelta::Sparse(v) => v.len(),
            ChunkDelta::Palette(p) => p.count,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The block stored at `index`, if modified.
    pub fn get(&self, index: usize) -> Option<BlockId> {
        match self {
            ChunkDelta::Sparse(v) => v
                .binary_search_by_key(&(index as u16), |e| e.0)
                .ok()
                .map(|k| v[k].1),
            ChunkDelta::Palette(p) => p.get(index),
        }
    }

    fn set(&mut self, index: usize, id: BlockId, palette_above: usize) {
        match self {
            ChunkDelta::Sparse(v) => {
                match v.binary_search_by_key(&(index as u16), |e| e.0) {
                    Ok(k) => v[k].1 = id,
                    Err(k) => v.insert(k, (index as u16, id)),
                }
                if v.len() > palette_above {
                    *self = ChunkDelta::Palette(Self::to_palette(v));
                }
            }
            ChunkDelta::Palette(p) => p.set(index, id),
        }
    }

    fn to_palette(v: &[(u16, BlockId)]) -> PaletteDelta {
        let distinct: BTreeSet<BlockId> = v.iter().map(|e| e.1).collect();
        let mut p = PaletteDelta::new(bits_for(distinct.len() + 1));
        for &(i, b) in v {
            p.set(usize::from(i), b);
        }
        p
    }

    /// Modified blocks `(index, block)`, by increasing index.
    pub fn entries(&self) -> Vec<(u16, BlockId)> {
        match self {
            ChunkDelta::Sparse(v) => v.clone(),
            ChunkDelta::Palette(p) => (0..VOLUME)
                .filter_map(|i| p.get(i).map(|b| (i as u16, b)))
                .collect(),
        }
    }
}

/// Local coordinates of a chunk index.
fn local_of(index: usize) -> (i32, i32, i32) {
    let x = index % N;
    let z = (index / N) % N;
    let y = index / (N * N);
    (x as i32, y as i32, z as i32)
}

/// Region of a chunk column.
pub fn region_of(cx: i32, cz: i32) -> (i32, i32) {
    (
        cx.rem_euclid(CHUNKS_PER_AXIS) / REGION_CHUNKS,
        cz.rem_euclid(CHUNKS_PER_AXIS) / REGION_CHUNKS,
    )
}

/// Every player modification of a world, with its region files.
#[derive(Clone, Debug)]
pub struct DeltaStore {
    /// Keyed `(cx, cz, cy)`: the chunks of a column are contiguous.
    chunks: BTreeMap<(i32, i32, i32), ChunkDelta>,
    dirty: BTreeSet<(i32, i32)>,
    dir: Option<PathBuf>,
    generator_version: u32,
    world_hash: String,
    palette_above: usize,
}

impl DeltaStore {
    /// An empty store, kept in memory only.
    pub fn new(generator_version: u32, world_hash: &str, palette_above: usize) -> Self {
        Self {
            chunks: BTreeMap::new(),
            dirty: BTreeSet::new(),
            dir: None,
            generator_version,
            world_hash: world_hash.to_owned(),
            palette_above,
        }
    }

    /// Loads every region file of `dir` (created if missing); [`save`](Self::save) writes there.
    ///
    /// Fails when a file was made for another world (other hash). Files from another
    /// generator version are loaded (absolute values) and listed in the returned warnings.
    pub fn open(
        dir: &Path,
        generator_version: u32,
        world_hash: &str,
        blocks: &Blocks,
        palette_above: usize,
    ) -> anyhow::Result<(Self, Vec<String>)> {
        let mut s = Self::new(generator_version, world_hash, palette_above);
        let mut warnings = Vec::new();
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "bin"))
            .collect();
        files.sort();
        for path in files {
            let bytes = std::fs::read(&path)?;
            let version = s
                .read_region(&bytes, blocks)
                .with_context(|| format!("reading {}", path.display()))?;
            if version != generator_version {
                warnings.push(format!(
                    "{}: made by generator version {version} (current {generator_version})",
                    path.display()
                ));
            }
        }
        s.dir = Some(dir.to_owned());
        Ok((s, warnings))
    }

    /// The modified block at `p`, if any.
    pub fn get(&self, p: BlockPos) -> Option<BlockId> {
        let c = p.chunk();
        self.chunks
            .get(&(c.cx, c.cz, c.cy))
            .and_then(|d| d.get(p.local().index()))
    }

    /// Records `id` at `p` (its region becomes dirty).
    pub fn set(&mut self, p: BlockPos, id: BlockId) {
        let c = p.chunk();
        self.chunks
            .entry((c.cx, c.cz, c.cy))
            .or_insert_with(|| ChunkDelta::Sparse(Vec::new()))
            .set(p.local().index(), id, self.palette_above);
        self.dirty.insert(region_of(c.cx, c.cz));
    }

    /// Whether column `(cx, cz)` holds a modified block.
    pub fn column_modified(&self, cx: i32, cz: i32) -> bool {
        let n = ChunkPos::new(cx, 0, cz);
        self.chunks
            .range((n.cx, n.cz, i32::MIN)..=(n.cx, n.cz, i32::MAX))
            .any(|(_, d)| !d.is_empty())
    }

    /// The delta of a chunk.
    pub fn chunk(&self, pos: ChunkPos) -> Option<&ChunkDelta> {
        self.chunks.get(&(pos.cx, pos.cz, pos.cy))
    }

    /// Number of modified chunks.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Heights of the chunks of column `(cx, cz)` whose mesh a delta may change: chunks with a
    /// delta in this column or a neighbouring one, and the chunks just above and below.
    pub fn touched_ys(&self, cx: i32, cz: i32) -> BTreeSet<i32> {
        let mut ys = BTreeSet::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                let n = ChunkPos::new(cx + dx, 0, cz + dz);
                for &(_, _, cy) in self
                    .chunks
                    .range((n.cx, n.cz, i32::MIN)..=(n.cx, n.cz, i32::MAX))
                    .map(|e| e.0)
                {
                    ys.extend(cy - 1..=cy + 1);
                }
            }
        }
        ys
    }

    /// Applies the deltas of the chunk and of its 26 neighbours (the one-block border).
    pub fn apply_padded(&self, pos: ChunkPos, chunk: &mut PaddedChunk) {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let Some(d) = self.chunk(pos.offset(dx, dy, dz)) else {
                        continue;
                    };
                    let (ox, oy, oz) = (dx * CHUNK_SIZE, dy * CHUNK_SIZE, dz * CHUNK_SIZE);
                    let inside = |v: i32| (-1..=CHUNK_SIZE).contains(&v);
                    for (i, b) in d.entries() {
                        let (lx, ly, lz) = local_of(usize::from(i));
                        let (x, y, z) = (lx + ox, ly + oy, lz + oz);
                        if inside(x) && inside(y) && inside(z) {
                            chunk.set(x, y, z, b);
                        }
                    }
                }
            }
        }
    }

    /// Writes the dirty regions (nothing for an in-memory store); returns how many.
    pub fn save(&mut self, blocks: &Blocks) -> anyhow::Result<usize> {
        let Some(dir) = self.dir.clone() else {
            self.dirty.clear();
            return Ok(0);
        };
        let dirty = std::mem::take(&mut self.dirty);
        for &(rx, rz) in &dirty {
            let path = dir.join(format!("r.{rx}.{rz}.bin"));
            let bytes = self.write_region(rx, rz, blocks);
            let tmp = path.with_extension("bin.tmp");
            std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
            std::fs::rename(&tmp, &path)
                .with_context(|| format!("replacing {}", path.display()))?;
        }
        Ok(dirty.len())
    }

    /// Region file: header (magic, format, generator version, world hash, block names), then
    /// the chunks of the region. Little-endian.
    fn write_region(&self, rx: i32, rz: i32, blocks: &Blocks) -> Vec<u8> {
        let chunks: Vec<(&(i32, i32, i32), &ChunkDelta)> = self
            .chunks
            .iter()
            .filter(|(k, d)| region_of(k.0, k.1) == (rx, rz) && !d.is_empty())
            .collect();
        // Block names, numbered in the file by first use.
        let mut names: Vec<BlockId> = Vec::new();
        let mut file_id = |b: BlockId| -> u16 {
            if b == KEEP {
                return KEEP;
            }
            match names.iter().position(|&n| n == b) {
                Some(k) => k as u16,
                None => {
                    names.push(b);
                    (names.len() - 1) as u16
                }
            }
        };
        let mut body = Vec::new();
        put_u32(&mut body, chunks.len() as u32);
        for (&(cx, cz, cy), d) in chunks {
            for v in [cx, cy, cz] {
                body.extend_from_slice(&v.to_le_bytes());
            }
            match d {
                ChunkDelta::Sparse(v) => {
                    body.push(0);
                    put_u32(&mut body, v.len() as u32);
                    for &(i, b) in v {
                        put_u16(&mut body, i);
                        put_u16(&mut body, file_id(b));
                    }
                }
                ChunkDelta::Palette(p) => {
                    body.push(1);
                    put_u16(&mut body, p.palette.len() as u16);
                    for &b in &p.palette {
                        put_u16(&mut body, file_id(b));
                    }
                    body.push(p.bits);
                    put_u32(&mut body, p.count as u32);
                    for &w in &p.words {
                        body.extend_from_slice(&w.to_le_bytes());
                    }
                }
            }
        }
        let mut out = Vec::with_capacity(body.len() + 256);
        out.extend_from_slice(MAGIC);
        put_u32(&mut out, FORMAT);
        put_u32(&mut out, self.generator_version);
        put_str(&mut out, &self.world_hash);
        put_u16(&mut out, names.len() as u16);
        for &b in &names {
            put_str(&mut out, &blocks.def(b).name);
        }
        out.extend_from_slice(&body);
        out
    }

    /// Reads a region file into the store; returns its generator version.
    fn read_region(&mut self, bytes: &[u8], blocks: &Blocks) -> anyhow::Result<u32> {
        let mut r = Reader(bytes);
        ensure!(r.take(8)? == MAGIC, "not a delta region file");
        let format = r.u32()?;
        ensure!(format == FORMAT, "unknown delta format {format}");
        let version = r.u32()?;
        let hash = r.str()?;
        if hash != self.world_hash {
            bail!("deltas made for another world (hash {hash})");
        }
        let n = r.u16()?;
        let mut ids = Vec::with_capacity(usize::from(n));
        for _ in 0..n {
            let name = r.str()?;
            ids.push(
                blocks
                    .id(&name)
                    .with_context(|| format!("unknown block `{name}`"))?,
            );
        }
        let id = |f: u16| -> anyhow::Result<BlockId> {
            if f == KEEP {
                return Ok(KEEP);
            }
            ids.get(usize::from(f))
                .copied()
                .with_context(|| format!("bad block number {f}"))
        };
        for _ in 0..r.u32()? {
            let (cx, cy, cz) = (r.i32()?, r.i32()?, r.i32()?);
            let pos = ChunkPos::new(cx, cy, cz);
            let d = match r.u8()? {
                0 => {
                    let n = r.u32()? as usize;
                    let mut v = Vec::with_capacity(n);
                    for _ in 0..n {
                        let i = r.u16()?;
                        ensure!(usize::from(i) < VOLUME, "bad block index {i}");
                        v.push((i, id(r.u16()?)?));
                    }
                    v.sort_unstable_by_key(|e| e.0);
                    v.dedup_by_key(|e| e.0);
                    ChunkDelta::Sparse(v)
                }
                1 => {
                    let n = r.u16()?;
                    let mut palette = Vec::with_capacity(usize::from(n));
                    for _ in 0..n {
                        palette.push(id(r.u16()?)?);
                    }
                    let bits = r.u8()?;
                    ensure!(
                        [1, 2, 4, 8, 16].contains(&bits) && palette.first() == Some(&KEEP),
                        "bad palette"
                    );
                    let count = r.u32()? as usize;
                    let mut words = Vec::with_capacity(VOLUME * usize::from(bits) / 64);
                    for _ in 0..VOLUME * usize::from(bits) / 64 {
                        words.push(u64::from_le_bytes(r.take(8)?.try_into()?));
                    }
                    let p = PaletteDelta {
                        palette,
                        bits,
                        words,
                        count,
                    };
                    ensure!(
                        (0..VOLUME).all(|i| p.slot(i) < p.palette.len()),
                        "bad palette index"
                    );
                    ChunkDelta::Palette(p)
                }
                k => bail!("unknown chunk kind {k}"),
            };
            self.chunks.insert((pos.cx, pos.cz, pos.cy), d);
        }
        ensure!(r.0.is_empty(), "trailing bytes");
        Ok(version)
    }
}

impl ChunkDeltas for DeltaStore {
    fn apply(&self, pos: ChunkPos, chunk: &mut Chunk) {
        if let Some(d) = self.chunk(pos) {
            for (i, b) in d.entries() {
                chunk.blocks_mut()[usize::from(i)] = b;
            }
        }
    }
}

fn put_u16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn put_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn put_str(v: &mut Vec<u8>, s: &str) {
    put_u16(v, s.len() as u16);
    v.extend_from_slice(s.as_bytes());
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        ensure!(self.0.len() >= n, "truncated file");
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> anyhow::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> anyhow::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn i32(&mut self) -> anyhow::Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn str(&mut self) -> anyhow::Result<String> {
        let n = self.u16()?;
        Ok(std::str::from_utf8(self.take(usize::from(n))?)?.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::AIR;
    use nascio_core::coords::LocalPos;

    fn blocks() -> Blocks {
        Blocks::builtin()
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nascio-delta-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn set_get_and_switch_to_palette() {
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let mut s = DeltaStore::new(1, "h", 100);
        let base = BlockPos::new(64, 10, 99_990);
        for k in 0..300 {
            s.set(
                base.offset(k % 8, k / 64, (k / 8) % 8),
                if k % 3 == 0 { AIR } else { stone },
            );
        }
        let c = base.chunk();
        assert!(matches!(s.chunk(c), Some(ChunkDelta::Palette(_))));
        assert_eq!(s.chunk(c).unwrap().len(), 300);
        for k in 0..300 {
            let want = if k % 3 == 0 { AIR } else { stone };
            assert_eq!(s.get(base.offset(k % 8, k / 64, (k / 8) % 8)), Some(want));
        }
        assert_eq!(s.get(base.offset(20, 0, 0)), None);
        // Growing the palette keeps every block.
        for (k, name) in ["sand", "dirt", "gravel", "snow"].iter().enumerate() {
            s.set(base.offset(9, k as i32, 0), b.id(name).unwrap());
        }
        assert_eq!(s.get(base.offset(9, 3, 0)), b.id("snow"));
        assert_eq!(s.get(base.offset(1, 0, 0)), Some(stone));
    }

    #[test]
    fn region_files_round_trip_across_the_seam() {
        let b = blocks();
        let d = dir("roundtrip");
        let (mut s, w) = DeltaStore::open(&d, 14, "abc", &b, 50).unwrap();
        assert!(w.is_empty() && s.chunk_count() == 0);
        // Both sides of the seam, and a chunk in palette form.
        s.set(BlockPos::new(99_999, 70, 5), AIR);
        s.set(BlockPos::new(0, -40, 99_999), b.id("ice").unwrap());
        for k in 0..60 {
            s.set(BlockPos::new(500 + k, 80, 500), b.id("granite").unwrap());
        }
        assert_eq!(s.save(&b).unwrap(), 3);
        let (back, w) = DeltaStore::open(&d, 14, "abc", &b, 50).unwrap();
        assert!(w.is_empty());
        assert_eq!(back.chunks, s.chunks);
        // Another generator version loads with a warning; another world fails.
        assert_eq!(DeltaStore::open(&d, 15, "abc", &b, 50).unwrap().1.len(), 3);
        assert!(DeltaStore::open(&d, 14, "xyz", &b, 50).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn padded_chunks_see_the_neighbours_deltas() {
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let mut s = DeltaStore::new(1, "h", DEFAULT_PALETTE_ABOVE);
        // Last column of the world, first block of the chunk above: border of chunk (0, 0, 0)
        // seen across the seam.
        s.set(BlockPos::new(99_999, 32, 0), stone);
        let mut p = PaddedChunk::filled(AIR);
        s.apply_padded(ChunkPos::new(0, 0, 0), &mut p);
        assert_eq!(p.get(-1, 32, 0), stone);
        assert_eq!(
            p.inner(),
            Chunk::filled(AIR),
            "the chunk itself is unchanged"
        );
        let ys = s.touched_ys(0, 0);
        assert_eq!(ys.into_iter().collect::<Vec<_>>(), vec![0, 1, 2]);
        // Plain chunks get the exact block.
        let mut c = Chunk::filled(AIR);
        s.apply(ChunkPos::new(CHUNKS_PER_AXIS - 1, 1, 0), &mut c);
        assert_eq!(c.get(LocalPos { x: 31, y: 0, z: 0 }), stone);
    }
}
