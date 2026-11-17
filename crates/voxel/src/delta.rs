//! Delta storage (spec §5: `Chunk = generate(pos) ⊕ delta(pos)`).
//!
//! A delta holds the modified blocks of a chunk as **absolute values** (the block now there,
//! not a difference from the generated one): a modification stays meaningful when the
//! generator changes. Each chunk keeps its modifications sorted by block index; above a
//! threshold they switch to a palette (block ids packed on a few bits, `KEEP` for unmodified
//! blocks).
//!
//! On disk, chunks are grouped in regions of `REGION_CHUNKS`² columns (all heights), in
//! `world-<seed>/deltas/`: a region file `r.<rx>.<rz>.bin` and an append-only journal
//! `r.<rx>.<rz>.log` of the edits made since the region file was last written (compaction).
//! Each file records the generator version and the hash of the world it was made for; region
//! files list the names of the blocks they use, journal records name their block (ids may
//! change between registries).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use anyhow::{Context, bail, ensure};
use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS};
use nascio_core::coords::{BlockPos, ChunkPos};
use serde::{Deserialize, Serialize};

use crate::block::{BlockId, Blocks};
use crate::chunk::{Chunk, ChunkDeltas, N, PaddedChunk, VOLUME};

/// Side of a region, in chunk columns (800 m). It divides [`CHUNKS_PER_AXIS`] (3125 = 5⁵),
/// so regions tile the torus.
pub const REGION_CHUNKS: i32 = 25;
const _: () = assert!(CHUNKS_PER_AXIS % REGION_CHUNKS == 0);

/// Default number of modified blocks above which a chunk switches to a palette.
pub const DEFAULT_PALETTE_ABOVE: usize = 2048;

const MAGIC: &[u8; 8] = b"NSCDELTA";
const LOG_MAGIC: &[u8; 8] = b"NSCDLOG\0";
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

/// Storage settings of a [`DeltaStore`]. They do not change the chunks: not part of the
/// world fingerprint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeltaConfig {
    /// Number of modified blocks above which a chunk switches to a palette.
    pub palette_above: usize,
    /// Journal records of a region above which it is compacted into its region file.
    pub compact_after: usize,
    /// Regions kept in memory (least recently used ones are dropped; their journal holds
    /// every edit). Not applied to in-memory stores.
    pub max_regions: usize,
}

impl Default for DeltaConfig {
    fn default() -> Self {
        Self {
            palette_above: DEFAULT_PALETTE_ABOVE,
            compact_after: 4096,
            max_regions: 64,
        }
    }
}

/// A region shared between threads, locked on its own.
type SharedRegion = Arc<RwLock<Region>>;

/// Modifications of one region, loaded on first use.
#[derive(Debug, Default)]
struct Region {
    loaded: bool,
    /// Keyed `(cx, cz, cy)`: the chunks of a column are contiguous. Each chunk is shared
    /// with its readers (snapshots) and copied only when edited while read.
    chunks: BTreeMap<(i32, i32, i32), Arc<ChunkDelta>>,
    /// Records in the journal since the last compaction.
    records: usize,
    journal: Option<File>,
    /// Why the region could not be read: it then reads as unmodified and refuses edits
    /// (its files are left untouched).
    broken: Option<String>,
}

#[derive(Debug, Default)]
struct Regions {
    /// Region and last use.
    map: BTreeMap<(i32, i32), (SharedRegion, u64)>,
    tick: u64,
}

/// Every player modification of a world, region by region.
///
/// Regions are loaded on first use (region file, then its journal) and locked
/// independently. An edit is appended to the region's journal before it is applied, and the
/// journal is compacted into the region file from time to time. Readers get one chunk at a
/// time ([`chunk`](Self::chunk)): an edit copies at most that chunk.
#[derive(Debug)]
pub struct DeltaStore {
    dir: Option<PathBuf>,
    generator_version: u32,
    world_hash: String,
    blocks: Blocks,
    cfg: DeltaConfig,
    regions: Mutex<Regions>,
    problems: Mutex<Vec<String>>,
}

impl DeltaStore {
    /// An empty store, kept in memory only.
    pub fn new(
        generator_version: u32,
        world_hash: &str,
        blocks: &Blocks,
        cfg: DeltaConfig,
    ) -> Self {
        Self {
            dir: None,
            generator_version,
            world_hash: world_hash.to_owned(),
            blocks: blocks.clone(),
            cfg,
            regions: Mutex::new(Regions::default()),
            problems: Mutex::new(Vec::new()),
        }
    }

    /// The store of `dir` (created if missing). Only the file headers are read here; regions
    /// load on first use.
    ///
    /// Fails when a file was made for another world (other hash). Files from another
    /// generator version are used (absolute values) and listed in the returned warnings.
    pub fn open(
        dir: &Path,
        generator_version: u32,
        world_hash: &str,
        blocks: &Blocks,
        cfg: DeltaConfig,
    ) -> anyhow::Result<(Self, Vec<String>)> {
        let mut s = Self::new(generator_version, world_hash, blocks, cfg);
        let mut warnings = Vec::new();
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "bin" || e == "log"))
            .collect();
        files.sort();
        for path in files {
            let magic = if path.extension().is_some_and(|e| e == "bin") {
                MAGIC
            } else {
                LOG_MAGIC
            };
            let version = read_header(&path, magic, world_hash)
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

    /// Problems met while loading regions (unreadable region, truncated journal), oldest
    /// first.
    pub fn problems(&self) -> Vec<String> {
        lock(&self.problems).clone()
    }

    fn problem(&self, msg: String) {
        lock(&self.problems).push(msg);
    }

    fn region_path(dir: &Path, (rx, rz): (i32, i32), ext: &str) -> PathBuf {
        dir.join(format!("r.{rx}.{rz}.{ext}"))
    }

    /// Region `r`, loaded.
    fn region(&self, r: (i32, i32)) -> Arc<RwLock<Region>> {
        let arc = {
            let mut regions = lock(&self.regions);
            regions.tick += 1;
            let tick = regions.tick;
            if let Some(e) = regions.map.get_mut(&r) {
                e.1 = tick;
                e.0.clone()
            } else {
                if self.dir.is_some() && regions.map.len() >= self.cfg.max_regions.max(1) {
                    // Least recently used region that nobody holds: its edits are in its files.
                    let old = regions
                        .map
                        .iter()
                        .filter(|(_, e)| Arc::strong_count(&e.0) == 1)
                        .min_by_key(|(_, e)| e.1)
                        .map(|(k, _)| *k);
                    if let Some(k) = old {
                        regions.map.remove(&k);
                    }
                }
                let arc = Arc::new(RwLock::new(Region::default()));
                regions.map.insert(r, (arc.clone(), tick));
                arc
            }
        };
        if !read(&arc).loaded {
            let mut g = write(&arc);
            if !g.loaded {
                self.load(r, &mut g);
            }
        }
        arc
    }

    fn load(&self, r: (i32, i32), g: &mut Region) {
        g.loaded = true;
        let Some(dir) = &self.dir else {
            return;
        };
        let bin = Self::region_path(dir, r, "bin");
        let log = Self::region_path(dir, r, "log");
        let result = (|| -> anyhow::Result<()> {
            if bin.exists() {
                let bytes = std::fs::read(&bin)?;
                self.read_region(&bytes, &mut g.chunks)
                    .with_context(|| format!("reading {}", bin.display()))?;
            }
            if log.exists() {
                let bytes = std::fs::read(&log)?;
                let (records, good) = self
                    .replay(&bytes, &mut g.chunks)
                    .with_context(|| format!("reading {}", log.display()))?;
                if good < bytes.len() {
                    // An edit cut short by a crash: drop it so that new records follow the
                    // last complete one.
                    self.problem(format!(
                        "{}: incomplete last record dropped ({} bytes)",
                        log.display(),
                        bytes.len() - good
                    ));
                    OpenOptions::new()
                        .write(true)
                        .open(&log)?
                        .set_len(good as u64)?;
                }
                g.records = records;
            }
            Ok(())
        })();
        if let Err(e) = result {
            g.chunks.clear();
            g.records = 0;
            let msg = format!("{e:#}");
            self.problem(msg.clone());
            g.broken = Some(msg);
        }
    }

    /// The delta of a chunk: a snapshot, unaffected by later edits.
    pub fn chunk(&self, pos: ChunkPos) -> Option<Arc<ChunkDelta>> {
        let r = self.region(region_of(pos.cx, pos.cz));
        let g = read(&r);
        g.chunks.get(&(pos.cx, pos.cz, pos.cy)).cloned()
    }

    /// The modified block at `p`, if any.
    pub fn get(&self, p: BlockPos) -> Option<BlockId> {
        self.chunk(p.chunk()).and_then(|d| d.get(p.local().index()))
    }

    /// Records `id` at `p`: appended to the region's journal, then applied.
    pub fn set(&self, p: BlockPos, id: BlockId) -> anyhow::Result<()> {
        let c = p.chunk();
        let rk = region_of(c.cx, c.cz);
        let r = self.region(rk);
        let mut g = write(&r);
        if let Some(e) = &g.broken {
            bail!("region {rk:?} unreadable, edit refused: {e}");
        }
        if let Some(dir) = &self.dir {
            if g.journal.is_none() {
                let path = Self::region_path(dir, rk, "log");
                let mut f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .with_context(|| format!("opening {}", path.display()))?;
                if f.metadata()?.len() == 0 {
                    f.write_all(&self.header(LOG_MAGIC))?;
                }
                g.journal = Some(f);
            }
            let mut rec = Vec::with_capacity(32);
            for v in [p.x, p.y, p.z] {
                rec.extend_from_slice(&v.to_le_bytes());
            }
            put_str(&mut rec, &self.blocks.def(id).name);
            if let Some(f) = g.journal.as_mut() {
                f.write_all(&rec).context("writing the journal")?;
            }
            g.records += 1;
        }
        let key = (c.cx, c.cz, c.cy);
        let d = g
            .chunks
            .entry(key)
            .or_insert_with(|| Arc::new(ChunkDelta::Sparse(Vec::new())));
        Arc::make_mut(d).set(p.local().index(), id, self.cfg.palette_above);
        if self.dir.is_some() && g.records > self.cfg.compact_after {
            self.compact(rk, &mut g)?;
        }
        Ok(())
    }

    /// Writes the region file of `rk` from memory, then drops its journal.
    fn compact(&self, rk: (i32, i32), g: &mut Region) -> anyhow::Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        let path = Self::region_path(dir, rk, "bin");
        let tmp = path.with_extension("bin.tmp");
        std::fs::write(&tmp, self.write_region(&g.chunks))
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        // A crash before this point replays the journal over the new file: same result
        // (absolute values).
        g.journal = None;
        let log = Self::region_path(dir, rk, "log");
        match std::fs::remove_file(&log) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("removing {}", log.display())),
        }
        g.records = 0;
        Ok(())
    }

    /// Compacts every loaded region that has journal records; returns how many.
    pub fn compact_all(&self) -> anyhow::Result<usize> {
        let loaded: Vec<(_, SharedRegion)> = lock(&self.regions)
            .map
            .iter()
            .map(|(k, e)| (*k, e.0.clone()))
            .collect();
        let mut n = 0;
        for (k, r) in loaded {
            let mut g = write(&r);
            if g.records > 0 && g.broken.is_none() {
                self.compact(k, &mut g)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Whether column `(cx, cz)` holds a modified block.
    pub fn column_modified(&self, cx: i32, cz: i32) -> bool {
        let n = ChunkPos::new(cx, 0, cz);
        let r = self.region(region_of(n.cx, n.cz));
        let g = read(&r);
        g.chunks
            .range((n.cx, n.cz, i32::MIN)..=(n.cx, n.cz, i32::MAX))
            .any(|(_, d)| !d.is_empty())
    }

    /// Number of modified chunks in the loaded regions.
    pub fn loaded_chunk_count(&self) -> usize {
        let loaded: Vec<Arc<RwLock<Region>>> = lock(&self.regions)
            .map
            .values()
            .map(|e| e.0.clone())
            .collect();
        loaded.iter().map(|r| read(r).chunks.len()).sum()
    }

    /// Heights of the chunks of column `(cx, cz)` whose mesh a delta may change: chunks with a
    /// delta in this column or a neighbouring one, and the chunks just above and below.
    pub fn touched_ys(&self, cx: i32, cz: i32) -> BTreeSet<i32> {
        let mut ys = BTreeSet::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                let n = ChunkPos::new(cx + dx, 0, cz + dz);
                let r = self.region(region_of(n.cx, n.cz));
                let g = read(&r);
                for &(_, _, cy) in g
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

    /// File header: magic, format, generator version, world hash.
    fn header(&self, magic: &[u8; 8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(magic);
        put_u32(&mut out, FORMAT);
        put_u32(&mut out, self.generator_version);
        put_str(&mut out, &self.world_hash);
        out
    }

    /// Region file: header, block names, then the chunks of the region. Little-endian.
    fn write_region(&self, chunks: &BTreeMap<(i32, i32, i32), Arc<ChunkDelta>>) -> Vec<u8> {
        let chunks: Vec<(_, &Arc<ChunkDelta>)> =
            chunks.iter().filter(|(_, d)| !d.is_empty()).collect();
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
            match &**d {
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
        let mut out = self.header(MAGIC);
        put_u16(&mut out, names.len() as u16);
        for &b in &names {
            put_str(&mut out, &self.blocks.def(b).name);
        }
        out.extend_from_slice(&body);
        out
    }

    /// Checks a header read by `r` (magic, format, world hash); returns its generator version.
    fn check_header(&self, r: &mut Reader, magic: &[u8; 8]) -> anyhow::Result<u32> {
        ensure!(r.take(8)? == magic, "not a delta file");
        let format = r.u32()?;
        ensure!(format == FORMAT, "unknown delta format {format}");
        let version = r.u32()?;
        let hash = r.str()?;
        if hash != self.world_hash {
            bail!("deltas made for another world (hash {hash})");
        }
        Ok(version)
    }

    /// Reads a region file into `chunks`.
    fn read_region(
        &self,
        bytes: &[u8],
        chunks: &mut BTreeMap<(i32, i32, i32), Arc<ChunkDelta>>,
    ) -> anyhow::Result<()> {
        let mut r = Reader(bytes);
        self.check_header(&mut r, MAGIC)?;
        let n = r.u16()?;
        let mut ids = Vec::with_capacity(usize::from(n));
        for _ in 0..n {
            let name = r.str()?;
            ids.push(
                self.blocks
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
            chunks.insert((pos.cx, pos.cz, pos.cy), Arc::new(d));
        }
        ensure!(r.0.is_empty(), "trailing bytes");
        Ok(())
    }

    /// Replays a journal into `chunks`; returns the number of records and the length of its
    /// complete part (a last record cut short is not applied).
    fn replay(
        &self,
        bytes: &[u8],
        chunks: &mut BTreeMap<(i32, i32, i32), Arc<ChunkDelta>>,
    ) -> anyhow::Result<(usize, usize)> {
        let mut r = Reader(bytes);
        self.check_header(&mut r, LOG_MAGIC)?;
        let mut records = 0;
        let mut good = bytes.len() - r.0.len();
        while !r.0.is_empty() {
            let rec = (|| -> anyhow::Result<(BlockPos, String)> {
                let (x, y, z) = (r.i32()?, r.i32()?, r.i32()?);
                Ok((BlockPos::new(x, y, z), r.str()?))
            })();
            let Ok((p, name)) = rec else {
                break;
            };
            let id = self
                .blocks
                .id(&name)
                .with_context(|| format!("unknown block `{name}`"))?;
            let c = p.chunk();
            let d = chunks
                .entry((c.cx, c.cz, c.cy))
                .or_insert_with(|| Arc::new(ChunkDelta::Sparse(Vec::new())));
            Arc::make_mut(d).set(p.local().index(), id, self.cfg.palette_above);
            records += 1;
            good = bytes.len() - r.0.len();
        }
        Ok((records, good))
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

/// Reads the header of a delta file; returns its generator version.
fn read_header(path: &Path, magic: &[u8; 8], world_hash: &str) -> anyhow::Result<u32> {
    let mut f = File::open(path)?;
    // Magic, format, version, hash length, then the hash.
    let mut head = vec![0u8; 18];
    f.read_exact(&mut head).context("truncated file")?;
    let n = usize::from(u16::from_le_bytes([head[16], head[17]]));
    head.resize(18 + n, 0);
    f.read_exact(&mut head[18..]).context("truncated file")?;
    let mut r = Reader(&head);
    ensure!(r.take(8)? == magic, "not a delta file");
    let format = r.u32()?;
    ensure!(format == FORMAT, "unknown delta format {format}");
    let version = r.u32()?;
    let hash = r.str()?;
    if hash != world_hash {
        bail!("deltas made for another world (hash {hash})");
    }
    Ok(version)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn read(r: &RwLock<Region>) -> RwLockReadGuard<'_, Region> {
    r.read().unwrap_or_else(|e| e.into_inner())
}

fn write(r: &RwLock<Region>) -> RwLockWriteGuard<'_, Region> {
    r.write().unwrap_or_else(|e| e.into_inner())
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
    use nascio_core::consts::WORLD_SIZE;
    use nascio_core::coords::LocalPos;
    use proptest::prelude::*;

    fn blocks() -> Blocks {
        Blocks::builtin()
    }

    fn cfg(palette_above: usize) -> DeltaConfig {
        DeltaConfig {
            palette_above,
            ..DeltaConfig::default()
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nascio-delta-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Every chunk of the loaded regions, for comparisons.
    fn all(s: &DeltaStore) -> BTreeMap<(i32, i32, i32), ChunkDelta> {
        let regions: Vec<Arc<RwLock<Region>>> =
            lock(&s.regions).map.values().map(|e| e.0.clone()).collect();
        regions
            .iter()
            .flat_map(|r| {
                read(r)
                    .chunks
                    .iter()
                    .map(|(k, d)| (*k, (**d).clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn set_get_and_switch_to_palette() {
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let s = DeltaStore::new(1, "h", &b, cfg(100));
        let base = BlockPos::new(64, 10, 99_990);
        for k in 0..300 {
            s.set(
                base.offset(k % 8, k / 64, (k / 8) % 8),
                if k % 3 == 0 { AIR } else { stone },
            )
            .unwrap();
        }
        let c = base.chunk();
        assert!(matches!(
            s.chunk(c).as_deref(),
            Some(ChunkDelta::Palette(_))
        ));
        assert_eq!(s.chunk(c).unwrap().len(), 300);
        for k in 0..300 {
            let want = if k % 3 == 0 { AIR } else { stone };
            assert_eq!(s.get(base.offset(k % 8, k / 64, (k / 8) % 8)), Some(want));
        }
        assert_eq!(s.get(base.offset(20, 0, 0)), None);
        // Growing the palette keeps every block.
        for (k, name) in ["sand", "dirt", "gravel", "snow"].iter().enumerate() {
            s.set(base.offset(9, k as i32, 0), b.id(name).unwrap())
                .unwrap();
        }
        assert_eq!(s.get(base.offset(9, 3, 0)), b.id("snow"));
        assert_eq!(s.get(base.offset(1, 0, 0)), Some(stone));
    }

    #[test]
    fn journal_and_region_files_round_trip_across_the_seam() {
        let b = blocks();
        let d = dir("roundtrip");
        let (s, w) = DeltaStore::open(&d, 14, "abc", &b, cfg(50)).unwrap();
        assert!(w.is_empty() && s.loaded_chunk_count() == 0);
        // Both sides of the seam, and a chunk in palette form.
        s.set(BlockPos::new(99_999, 70, 5), AIR).unwrap();
        s.set(BlockPos::new(0, -40, 99_999), b.id("ice").unwrap())
            .unwrap();
        for k in 0..60 {
            s.set(
                BlockPos::new(512 + k % 32, 80 + k / 32, 500),
                b.id("granite").unwrap(),
            )
            .unwrap();
        }
        // Journals only: reopened, the store replays them.
        let (back, w) = DeltaStore::open(&d, 14, "abc", &b, cfg(50)).unwrap();
        assert!(w.is_empty());
        for p in [
            BlockPos::new(99_999, 70, 5),
            BlockPos::new(0, -40, 99_999),
            BlockPos::new(512, 80, 500),
        ] {
            back.chunk(p.chunk());
        }
        assert_eq!(all(&back), all(&s));
        // Compacted into region files (one per region), journals gone.
        assert_eq!(s.compact_all().unwrap(), 3);
        assert_eq!(s.compact_all().unwrap(), 0);
        let logs = std::fs::read_dir(&d)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension().unwrap() == "log")
            .count();
        assert_eq!(logs, 0);
        let (back, _) = DeltaStore::open(&d, 14, "abc", &b, cfg(50)).unwrap();
        assert_eq!(back.get(BlockPos::new(-1, 70, 5)), Some(AIR));
        assert!(matches!(
            back.chunk(BlockPos::new(512, 80, 500).chunk()).as_deref(),
            Some(ChunkDelta::Palette(_))
        ));
        back.chunk(BlockPos::new(0, -40, 99_999).chunk());
        assert_eq!(all(&back), all(&s));
        // Another generator version loads with a warning; another world fails.
        assert_eq!(
            DeltaStore::open(&d, 15, "abc", &b, cfg(50))
                .unwrap()
                .1
                .len(),
            3
        );
        assert!(DeltaStore::open(&d, 14, "xyz", &b, cfg(50)).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn automatic_compaction_and_truncated_journal() {
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let d = dir("compact");
        let c = DeltaConfig {
            compact_after: 10,
            ..DeltaConfig::default()
        };
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, c.clone()).unwrap();
        for k in 0..13 {
            s.set(BlockPos::new(k, 5, 0), stone).unwrap();
        }
        // Compacted at the 11th record; two records in the new journal.
        let log = d.join("r.0.0.log");
        assert!(d.join("r.0.0.bin").exists());
        let len = std::fs::metadata(&log).unwrap().len();
        // A crash in the middle of the last record.
        let f = OpenOptions::new().write(true).open(&log).unwrap();
        f.set_len(len - 3).unwrap();
        drop(f);
        drop(s);
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, c).unwrap();
        assert_eq!(s.get(BlockPos::new(11, 5, 0)), Some(stone));
        assert_eq!(s.get(BlockPos::new(12, 5, 0)), None);
        assert_eq!(s.problems().len(), 1);
        // New records follow the last complete one.
        s.set(BlockPos::new(20, 5, 0), AIR).unwrap();
        drop(s);
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        assert_eq!(s.get(BlockPos::new(20, 5, 0)), Some(AIR));
        assert_eq!(s.get(BlockPos::new(11, 5, 0)), Some(stone));
        assert!(s.problems().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unreadable_region_refuses_edits_and_others_work() {
        let b = blocks();
        let d = dir("broken");
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        s.set(BlockPos::new(0, 0, 0), AIR).unwrap();
        s.compact_all().unwrap();
        drop(s);
        // Corrupt the body, keep the header.
        let p = d.join("r.0.0.bin");
        let mut bytes = std::fs::read(&p).unwrap();
        bytes.push(7);
        std::fs::write(&p, &bytes).unwrap();
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        assert_eq!(s.get(BlockPos::new(0, 0, 0)), None);
        assert!(s.set(BlockPos::new(1, 0, 0), AIR).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), bytes, "file left untouched");
        s.set(BlockPos::new(1_000, 0, 0), AIR).unwrap();
        assert_eq!(s.problems().len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn regions_beyond_the_bound_are_dropped_and_reloaded() {
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let d = dir("evict");
        let c = DeltaConfig {
            max_regions: 2,
            ..DeltaConfig::default()
        };
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, c).unwrap();
        let region_m = REGION_CHUNKS * CHUNK_SIZE;
        for k in 0..5 {
            s.set(BlockPos::new(k * region_m, 3, 7), stone).unwrap();
            assert!(lock(&s.regions).map.len() <= 2);
        }
        for k in 0..5 {
            assert_eq!(s.get(BlockPos::new(k * region_m, 3, 7)), Some(stone));
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn chunk_snapshots_are_not_changed_by_later_edits() {
        let b = blocks();
        let s = DeltaStore::new(1, "h", &b, DeltaConfig::default());
        let p = BlockPos::new(3, 3, 3);
        s.set(p, AIR).unwrap();
        let snap = s.chunk(p.chunk()).unwrap();
        s.set(p, b.id("sand").unwrap()).unwrap();
        assert_eq!(snap.get(p.local().index()), Some(AIR));
        assert_eq!(s.get(p), b.id("sand"));
    }

    #[test]
    fn regions_are_edited_concurrently() {
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let d = dir("threads");
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        let region_m = REGION_CHUNKS * CHUNK_SIZE;
        std::thread::scope(|sc| {
            for t in 0..4 {
                let s = &s;
                sc.spawn(move || {
                    for k in 0..200 {
                        s.set(BlockPos::new(t * region_m - 1, k, k), stone).unwrap();
                    }
                });
            }
        });
        drop(s);
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        for t in 0..4 {
            for k in 0..200 {
                assert_eq!(s.get(BlockPos::new(t * region_m - 1, k, k)), Some(stone));
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn padded_chunks_see_the_neighbours_deltas() {
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let s = DeltaStore::new(1, "h", &b, DeltaConfig::default());
        // Last column of the world, first block of the chunk above: border of chunk (0, 0, 0)
        // seen across the seam.
        s.set(BlockPos::new(99_999, 32, 0), stone).unwrap();
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

    /// An edit near the seam: offsets from the corner of the world (both sides), and the
    /// number of periods added to the coordinates (the same block).
    fn edit() -> impl Strategy<Value = (i32, i32, i32, i32, u8)> {
        (-40i32..40, -40i32..40, -70i32..70, -1i32..=1, 0u8..4)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// Across the seam, the store matches a plain map of wrapped positions, in memory,
        /// after compaction and after reopening, wherever the coordinates are written.
        #[test]
        fn store_matches_a_model_across_the_seam(
            edits in prop::collection::vec(edit(), 1..120),
            compact_at in 0usize..120,
            case in any::<u32>(),
        ) {
            let b = blocks();
            let ids = [AIR, b.id("granite").unwrap(), b.id("ice").unwrap(), b.id("sand").unwrap()];
            let d = dir(&format!("prop-{case}"));
            let c = DeltaConfig { palette_above: 16, compact_after: 25, max_regions: 2 };
            let (s, _) = DeltaStore::open(&d, 1, "h", &b, c.clone()).unwrap();
            let mut model: BTreeMap<(i32, i32, i32), BlockId> = BTreeMap::new();
            for (k, &(x, z, y, periods, id)) in edits.iter().enumerate() {
                let shift = periods * WORLD_SIZE;
                let p = BlockPos::new(x + shift, y, z - shift);
                s.set(p, ids[usize::from(id)]).unwrap();
                model.insert((x.rem_euclid(WORLD_SIZE), y, z.rem_euclid(WORLD_SIZE)), ids[usize::from(id)]);
                if k == compact_at {
                    s.compact_all().unwrap();
                }
            }
            let check = |s: &DeltaStore| -> Result<(), TestCaseError> {
                for x in -40..40 {
                    for z in [-40, -1, 0, 39] {
                        for y in [-70, -33, -1, 0, 31, 32, 69] {
                            let p = BlockPos::new(x, y, z);
                            let want = model.get(&(p.x, p.y, p.z)).copied();
                            prop_assert_eq!(s.get(p), want);
                            prop_assert_eq!(s.get(BlockPos::new(x - WORLD_SIZE, y, z + WORLD_SIZE)), want);
                        }
                    }
                }
                for (&(x, y, z), &id) in &model {
                    prop_assert_eq!(s.get(BlockPos::new(x, y, z)), Some(id));
                }
                Ok(())
            };
            check(&s)?;
            drop(s);
            let (s, _) = DeltaStore::open(&d, 1, "h", &b, c).unwrap();
            check(&s)?;
            prop_assert!(s.problems().is_empty());
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}
