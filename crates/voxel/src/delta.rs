//! Delta storage (spec §5: `Chunk = generate(pos) ⊕ delta(pos)`).
//!
//! A delta holds the modified blocks of a chunk as **absolute values** (the block now there,
//! not a difference from the generated one): a modification stays meaningful when the
//! generator changes. Each chunk keeps its modifications sorted by block index; above a
//! threshold they switch to a palette (block ids packed on a few bits, `KEEP` for unmodified
//! blocks).
//!
//! On disk, chunks are grouped in regions of `REGION_CHUNKS`² columns (all heights), in
//! `world-<seed>/deltas/`: a region file `r.<rx>.<rz>.bin` per region, and one global
//! append-only journal `wal/w.<first lsn>.log` (segments) of the edits made since the last
//! checkpoint, each record numbered (LSN) and checksummed (CRC-32C). `flush` forces the
//! journal to the disk (group commit); a checkpoint writes the modified regions (with the
//! number of their last record) and drops the old segments. Each file records the generator
//! version and the hash of the world it was made for; region files list the names of the
//! blocks they use, journal records name their block (ids may change between registries).

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
/// Region journal of format 1 (before the global journal), read once and migrated.
const LOG_MAGIC: &[u8; 8] = b"NSCDLOG\0";
const WAL_MAGIC: &[u8; 8] = b"NSCWAL\0\0";
/// Folder of the global journal, inside the deltas folder.
const WAL_DIR: &str = "wal";
/// Region file format: 2 adds the number of the last journal record applied.
const FORMAT: u32 = 2;
/// Journal record kind: a block edit. Other kinds are reserved for game state.
const REC_BLOCK: u8 = 1;
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
/// Writes `bytes` to `path` so that a crash or a power cut leaves either the old file or the
/// new one, complete: temporary file, `fsync`, rename, then `fsync` of the directory (Unix;
/// NTFS journals the rename itself and a directory cannot be opened as a file on Windows).
pub fn write_durable(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut name = path.file_name().context("file name")?.to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let mut f = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    f.write_all(bytes)
        .with_context(|| format!("writing {}", tmp.display()))?;
    f.sync_all()
        .with_context(|| format!("syncing {}", tmp.display()))?;
    drop(f);
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    sync_dir(path.parent().unwrap_or(Path::new(".")))
}

/// Makes the names of a directory durable (renames, creations, removals).
pub fn sync_dir(dir: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    File::open(dir)
        .and_then(|d| d.sync_all())
        .with_context(|| format!("syncing {}", dir.display()))?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

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
    /// Size of the journal (bytes, all segments) above which a checkpoint writes the modified
    /// regions to their files and drops the old segments.
    pub checkpoint_bytes: u64,
    /// Regions kept in memory (least recently used ones are dropped, written first when
    /// modified). Not applied to in-memory stores.
    pub max_regions: usize,
}

impl Default for DeltaConfig {
    fn default() -> Self {
        Self {
            palette_above: DEFAULT_PALETTE_ABOVE,
            checkpoint_bytes: 16 << 20,
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
    /// Sequence number of the last journal record applied (from the file, then the edits).
    lsn: u64,
    /// Holds edits that are not in the region file yet (only in the journal).
    dirty: bool,
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

/// An edit read from the journal, waiting for its region to load.
#[derive(Clone, Copy, Debug)]
struct Pending {
    lsn: u64,
    pos: BlockPos,
    id: BlockId,
}

/// The global journal: segments `wal/w.<first lsn>.log`, records numbered by `lsn`.
#[derive(Debug, Default)]
struct Wal {
    /// Segment being appended to, and its path.
    file: Option<(File, PathBuf)>,
    /// Every segment on the disk, by first sequence number.
    segments: BTreeMap<u64, PathBuf>,
    /// Number of the next record.
    next_lsn: u64,
    /// Bytes of every segment.
    bytes: u64,
    /// Last record forced to the disk.
    synced_lsn: u64,
}

/// Every player modification of a world, region by region.
///
/// Regions are loaded on first use (region file, then their records of the journal) and
/// locked independently. An edit is written to the global journal (the OS has it: it
/// survives a crash of the program) before it is applied; [`flush`](Self::flush) forces the
/// journal to the disk (one `fsync` for every edit since the previous one: group commit).
/// Checkpoints write the modified regions to their files and drop the old journal segments.
/// Readers get one chunk at a time ([`chunk`](Self::chunk)): an edit copies at most that
/// chunk.
#[derive(Debug)]
pub struct DeltaStore {
    dir: Option<PathBuf>,
    generator_version: u32,
    world_hash: String,
    blocks: Blocks,
    cfg: DeltaConfig,
    regions: Mutex<Regions>,
    wal: Mutex<Wal>,
    /// Journal records of regions not loaded yet, by region.
    pending: Mutex<BTreeMap<(i32, i32), Vec<Pending>>>,
    /// Held during a checkpoint (one at a time).
    checkpointing: Mutex<()>,
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
            wal: Mutex::new(Wal {
                next_lsn: 1,
                ..Wal::default()
            }),
            pending: Mutex::new(BTreeMap::new()),
            checkpointing: Mutex::new(()),
            problems: Mutex::new(Vec::new()),
        }
    }

    /// The store of `dir` (created if missing). Region files are read on first use; only
    /// their headers are read here, with the whole journal (indexed by region).
    ///
    /// Fails when a file was made for another world (other hash) or when the journal is
    /// corrupt before its last record. A last record cut short (crash) is dropped and listed
    /// in [`problems`](Self::problems). Files from another generator version are used
    /// (absolute values) and listed in the returned warnings.
    pub fn open(
        dir: &Path,
        generator_version: u32,
        world_hash: &str,
        blocks: &Blocks,
        cfg: DeltaConfig,
    ) -> anyhow::Result<(Self, Vec<String>)> {
        let s = Self::new(generator_version, world_hash, blocks, cfg);
        let mut warnings = Vec::new();
        let wal_dir = dir.join(WAL_DIR);
        std::fs::create_dir_all(&wal_dir)
            .with_context(|| format!("creating {}", wal_dir.display()))?;
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "bin" || e == "log"))
            .collect();
        files.sort();
        let mut max_lsn = 0;
        for path in files {
            let magic = if path.extension().is_some_and(|e| e == "bin") {
                MAGIC
            } else {
                LOG_MAGIC
            };
            let (version, lsn) = read_header(&path, magic, world_hash)
                .with_context(|| format!("reading {}", path.display()))?;
            max_lsn = max_lsn.max(lsn);
            if version != generator_version {
                warnings.push(format!(
                    "{}: made by generator version {version} (current {generator_version})",
                    path.display()
                ));
            }
        }
        // The journal, segment by segment.
        let mut segments = BTreeMap::new();
        for e in std::fs::read_dir(&wal_dir)? {
            let path = e?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if let Some(hex) = name.strip_prefix("w.").and_then(|n| n.strip_suffix(".log")) {
                let first = u64::from_str_radix(hex, 16)
                    .with_context(|| format!("bad segment name {}", path.display()))?;
                segments.insert(first, path);
            }
        }
        let mut wal = Wal::default();
        let mut pending: BTreeMap<(i32, i32), Vec<Pending>> = BTreeMap::new();
        let mut expect: Option<u64> = None;
        let last = segments.keys().next_back().copied();
        for (&first, path) in &segments {
            let bytes = std::fs::read(path)?;
            let mut r = Reader(&bytes);
            let version = s
                .check_header(&mut r, WAL_MAGIC)
                .with_context(|| format!("reading {}", path.display()))?;
            if version != generator_version {
                warnings.push(format!(
                    "{}: made by generator version {version} (current {generator_version})",
                    path.display()
                ));
            }
            let mut good = bytes.len() - r.0.len();
            let mut lsn = expect.unwrap_or(first);
            ensure!(
                lsn == first,
                "{}: journal segment out of sequence (expected record {lsn})",
                path.display()
            );
            loop {
                if r.0.is_empty() {
                    break;
                }
                match s.read_record(&mut r, lsn) {
                    Ok(p) => {
                        pending
                            .entry(region_of(p.pos.chunk().cx, p.pos.chunk().cz))
                            .or_default()
                            .push(p);
                        lsn += 1;
                        good = bytes.len() - r.0.len();
                    }
                    Err(e) if Some(first) == last => {
                        // Cut short by a crash: drop it and anything after it.
                        s.problem(format!(
                            "{}: last record dropped ({} bytes): {e:#}",
                            path.display(),
                            bytes.len() - good
                        ));
                        OpenOptions::new()
                            .write(true)
                            .open(path)?
                            .set_len(good as u64)?;
                        break;
                    }
                    Err(e) => {
                        return Err(e).with_context(|| {
                            format!("journal corrupt in {} at byte {good}", path.display())
                        });
                    }
                }
            }
            expect = Some(lsn);
            wal.bytes += good as u64;
            max_lsn = max_lsn.max(lsn - 1);
        }
        wal.next_lsn = max_lsn + 1;
        wal.synced_lsn = max_lsn;
        if let Some((&first, path)) = segments.iter().next_back() {
            let f = OpenOptions::new()
                .append(true)
                .open(path)
                .with_context(|| format!("opening {}", path.display()))?;
            // Records continue the last segment only if their numbers follow on.
            if expect == Some(wal.next_lsn) || first == wal.next_lsn {
                wal.file = Some((f, path.clone()));
            }
        }
        wal.segments = segments;
        let mut s = s;
        *lock(&s.wal) = wal;
        *lock(&s.pending) = pending;
        s.dir = Some(dir.to_owned());
        Ok((s, warnings))
    }

    /// Problems met while loading (unreadable region, record cut short), oldest first.
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
                    // Least recently used region that nobody holds, written first if modified.
                    let mut idle: Vec<((i32, i32), u64)> = regions
                        .map
                        .iter()
                        .filter(|(_, e)| Arc::strong_count(&e.0) == 1)
                        .map(|(k, e)| (*k, e.1))
                        .collect();
                    idle.sort_unstable_by_key(|e| e.1);
                    for (k, _) in idle {
                        let ok = {
                            let mut g = write(&regions.map[&k].0);
                            match self.write_back(k, &mut g) {
                                Ok(()) => true,
                                Err(e) => {
                                    self.problem(format!("keeping region {k:?} loaded: {e:#}"));
                                    false
                                }
                            }
                        };
                        if ok {
                            regions.map.remove(&k);
                            break;
                        }
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

    /// Writes a modified region to its file (durably), with the number of its last record.
    fn write_back(&self, rk: (i32, i32), g: &mut Region) -> anyhow::Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        if !g.dirty || g.broken.is_some() {
            return Ok(());
        }
        let path = Self::region_path(dir, rk, "bin");
        write_durable(&path, &self.write_region(&g.chunks, g.lsn))?;
        g.dirty = false;
        Ok(())
    }

    fn load(&self, r: (i32, i32), g: &mut Region) {
        g.loaded = true;
        let Some(dir) = &self.dir else {
            return;
        };
        let bin = Self::region_path(dir, r, "bin");
        let log = Self::region_path(dir, r, "log");
        let mut chunks = BTreeMap::new();
        let result = (|| -> anyhow::Result<(u64, bool)> {
            let mut lsn = 0;
            if bin.exists() {
                let bytes = std::fs::read(&bin)?;
                lsn = self
                    .read_region(&bytes, &mut chunks)
                    .with_context(|| format!("reading {}", bin.display()))?;
            }
            // Journal of the region (format before the global journal): older than every
            // record of the global journal.
            let migrated = log.exists();
            if migrated {
                let bytes = std::fs::read(&log)?;
                let good = self
                    .replay_region_log(&bytes, &mut chunks)
                    .with_context(|| format!("reading {}", log.display()))?;
                if good < bytes.len() {
                    self.problem(format!(
                        "{}: incomplete last record dropped ({} bytes)",
                        log.display(),
                        bytes.len() - good
                    ));
                }
            }
            Ok((lsn, migrated))
        })();
        match result {
            Ok((file_lsn, migrated)) => {
                g.lsn = file_lsn;
                let records = lock(&self.pending).remove(&r).unwrap_or_default();
                for p in records.iter().filter(|p| p.lsn > file_lsn) {
                    set_in(&mut chunks, p.pos, p.id, self.cfg.palette_above);
                    g.lsn = p.lsn;
                    g.dirty = true;
                }
                g.chunks = chunks;
                if migrated {
                    g.dirty = true;
                    let done = self.write_back(r, g).and_then(|()| {
                        std::fs::remove_file(&log)
                            .with_context(|| format!("removing {}", log.display()))?;
                        sync_dir(dir)
                    });
                    if let Err(e) = done {
                        self.problem(format!("migrating {}: {e:#}", log.display()));
                    }
                }
            }
            Err(e) => {
                // Its journal records stay pending: a checkpoint keeps their segments.
                let msg = format!("{e:#}");
                self.problem(msg.clone());
                g.broken = Some(msg);
            }
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

    /// Sets a block: the edit is written to the journal (it survives a crash of the program;
    /// a power cut only after the next [`flush`](Self::flush)), then applied. Returns its
    /// sequence number (0 for a store in memory).
    pub fn set(&self, p: BlockPos, id: BlockId) -> anyhow::Result<u64> {
        let c = p.chunk();
        let rk = region_of(c.cx, c.cz);
        let r = self.region(rk);
        let checkpoint;
        let lsn;
        {
            let mut g = write(&r);
            if let Some(e) = &g.broken {
                bail!("region {rk:?} unreadable, edit refused: {e}");
            }
            {
                let mut w = lock(&self.wal);
                lsn = w.next_lsn;
                if self.dir.is_some() {
                    let rec = self.record(lsn, p, id);
                    self.append(&mut w, &rec)?;
                }
                w.next_lsn += 1;
                checkpoint = self.dir.is_some() && w.bytes > self.cfg.checkpoint_bytes;
            }
            set_in(&mut g.chunks, p, id, self.cfg.palette_above);
            g.lsn = lsn;
            g.dirty = true;
        }
        drop(r);
        if checkpoint {
            self.checkpoint()?;
        }
        Ok(if self.dir.is_some() { lsn } else { 0 })
    }

    /// Appends a record to the current segment (opened or created as needed).
    fn append(&self, w: &mut Wal, rec: &[u8]) -> anyhow::Result<()> {
        if w.file.is_none() {
            let dir = self.dir.as_ref().context("store in memory")?.join(WAL_DIR);
            let path = dir.join(format!("w.{:016x}.log", w.next_lsn));
            write_durable(&path, &self.header(WAL_MAGIC))?;
            let f = OpenOptions::new()
                .append(true)
                .open(&path)
                .with_context(|| format!("opening {}", path.display()))?;
            w.bytes += f.metadata()?.len();
            w.segments.insert(w.next_lsn, path.clone());
            w.file = Some((f, path));
        }
        let (f, path) = w.file.as_mut().expect("segment open");
        f.write_all(rec)
            .with_context(|| format!("writing {}", path.display()))?;
        w.bytes += rec.len() as u64;
        Ok(())
    }

    /// Forces every edit made so far to the disk (one `fsync` of the journal: group commit).
    /// Returns the sequence number of the last durable edit.
    pub fn flush(&self) -> anyhow::Result<u64> {
        let mut w = lock(&self.wal);
        let last = w.next_lsn - 1;
        if w.synced_lsn < last {
            if let Some((f, path)) = &w.file {
                f.sync_data()
                    .with_context(|| format!("syncing {}", path.display()))?;
            }
            w.synced_lsn = last;
        }
        Ok(last)
    }

    /// Writes every modified region to its file, then drops the journal segments they cover;
    /// returns the number of regions written. Called automatically when the journal exceeds
    /// [`DeltaConfig::checkpoint_bytes`]. Segments are kept while a region that has records
    /// in them cannot be read.
    pub fn checkpoint(&self) -> anyhow::Result<usize> {
        let Some(dir) = &self.dir else {
            return Ok(0);
        };
        let _one = lock(&self.checkpointing);
        // New segment: the old ones only hold edits made before this point.
        let old: Vec<PathBuf> = {
            let mut w = lock(&self.wal);
            if let Some((f, path)) = &w.file {
                f.sync_data()
                    .with_context(|| format!("syncing {}", path.display()))?;
            }
            w.synced_lsn = w.next_lsn - 1;
            w.file = None;
            let old: Vec<PathBuf> = std::mem::take(&mut w.segments).into_values().collect();
            w.bytes = 0;
            old
        };
        // Regions with records in the old segments: loaded and modified, or not loaded yet.
        let waiting: Vec<(i32, i32)> = lock(&self.pending).keys().copied().collect();
        let loaded: Vec<((i32, i32), SharedRegion)> = lock(&self.regions)
            .map
            .iter()
            .map(|(k, e)| (*k, e.0.clone()))
            .collect();
        let mut n = 0;
        let mut keep = false;
        let mut write_one = |k: (i32, i32), r: &SharedRegion| -> anyhow::Result<()> {
            let mut g = write(r);
            if g.broken.is_some() {
                keep |= lock(&self.pending).contains_key(&k);
                return Ok(());
            }
            if g.dirty {
                self.write_back(k, &mut g)?;
                n += 1;
            }
            Ok(())
        };
        for (k, r) in &loaded {
            write_one(*k, r)?;
        }
        for k in waiting {
            let r = self.region(k);
            write_one(k, &r)?;
        }
        if keep {
            // A broken region still needs its records: the segments stay (listed again).
            let mut w = lock(&self.wal);
            for p in old {
                let first = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_prefix("w.")?.strip_suffix(".log"))
                    .and_then(|h| u64::from_str_radix(h, 16).ok())
                    .unwrap_or(0);
                w.bytes += std::fs::metadata(&p).map_or(0, |m| m.len());
                w.segments.insert(first, p);
            }
            self.problem("checkpoint: journal kept for an unreadable region".into());
            return Ok(n);
        }
        for p in old {
            std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
        }
        sync_dir(&dir.join(WAL_DIR))?;
        Ok(n)
    }

    /// Writes every modified region to its file and empties the journal; returns the number
    /// of regions written. Same as [`checkpoint`](Self::checkpoint).
    pub fn compact_all(&self) -> anyhow::Result<usize> {
        self.checkpoint()
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
        let format = if magic == MAGIC { FORMAT } else { 1 };
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(magic);
        put_u32(&mut out, format);
        put_u32(&mut out, self.generator_version);
        put_str(&mut out, &self.world_hash);
        out
    }

    /// Journal record: length, sequence number, kind, content, CRC-32C (of the number, the
    /// kind and the content).
    fn record(&self, lsn: u64, p: BlockPos, id: BlockId) -> Vec<u8> {
        let mut body = Vec::with_capacity(40);
        body.extend_from_slice(&lsn.to_le_bytes());
        body.push(REC_BLOCK);
        for v in [p.x, p.y, p.z] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        put_str(&mut body, &self.blocks.def(id).name);
        let mut rec = Vec::with_capacity(body.len() + 8);
        put_u32(&mut rec, body.len() as u32);
        rec.extend_from_slice(&body);
        put_u32(&mut rec, crc32c(&body));
        rec
    }

    /// Reads one journal record, which must be number `lsn`.
    fn read_record(&self, r: &mut Reader, lsn: u64) -> anyhow::Result<Pending> {
        let len = r.u32()? as usize;
        ensure!((9..=1 << 16).contains(&len), "bad record length {len}");
        let body = r.take(len)?;
        let crc = r.u32()?;
        ensure!(crc == crc32c(body), "bad checksum");
        let mut b = Reader(body);
        let n = u64::from_le_bytes(b.take(8)?.try_into()?);
        ensure!(n == lsn, "record {n} where {lsn} was expected");
        let kind = b.u8()?;
        ensure!(kind == REC_BLOCK, "unknown record kind {kind}");
        let (x, y, z) = (b.i32()?, b.i32()?, b.i32()?);
        let name = b.str()?;
        ensure!(b.0.is_empty(), "trailing bytes in record");
        let id = self
            .blocks
            .id(&name)
            .with_context(|| format!("unknown block `{name}`"))?;
        Ok(Pending {
            lsn,
            pos: BlockPos::new(x, y, z),
            id,
        })
    }

    /// Region file: header, number of the last journal record applied, block names, then the
    /// chunks of the region. Little-endian.
    fn write_region(
        &self,
        chunks: &BTreeMap<(i32, i32, i32), Arc<ChunkDelta>>,
        lsn: u64,
    ) -> Vec<u8> {
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
        out.extend_from_slice(&lsn.to_le_bytes());
        put_u16(&mut out, names.len() as u16);
        for &b in &names {
            put_str(&mut out, &self.blocks.def(b).name);
        }
        out.extend_from_slice(&body);
        out
    }

    /// Checks the header of a journal read by `r` (magic, format 1, world hash); returns its
    /// generator version.
    fn check_header(&self, r: &mut Reader, magic: &[u8; 8]) -> anyhow::Result<u32> {
        ensure!(r.take(8)? == magic, "not a delta file");
        let format = r.u32()?;
        ensure!(format == 1, "unknown delta format {format}");
        let version = r.u32()?;
        let hash = r.str()?;
        if hash != self.world_hash {
            bail!("deltas made for another world (hash {hash})");
        }
        Ok(version)
    }

    /// Reads a region file into `chunks`; returns the number of its last journal record.
    fn read_region(
        &self,
        bytes: &[u8],
        chunks: &mut BTreeMap<(i32, i32, i32), Arc<ChunkDelta>>,
    ) -> anyhow::Result<u64> {
        let mut r = Reader(bytes);
        ensure!(r.take(8)? == MAGIC, "not a delta file");
        let format = r.u32()?;
        ensure!(
            (1..=FORMAT).contains(&format),
            "unknown delta format {format}"
        );
        let _version = r.u32()?;
        let hash = r.str()?;
        if hash != self.world_hash {
            bail!("deltas made for another world (hash {hash})");
        }
        let lsn = if format >= 2 {
            u64::from_le_bytes(r.take(8)?.try_into()?)
        } else {
            0
        };
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
        Ok(lsn)
    }

    /// Replays a region journal of the format before the global journal into `chunks`;
    /// returns the length of its complete part (a last record cut short is not applied).
    fn replay_region_log(
        &self,
        bytes: &[u8],
        chunks: &mut BTreeMap<(i32, i32, i32), Arc<ChunkDelta>>,
    ) -> anyhow::Result<usize> {
        let mut r = Reader(bytes);
        self.check_header(&mut r, LOG_MAGIC)?;
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
            set_in(chunks, p, id, self.cfg.palette_above);
            good = bytes.len() - r.0.len();
        }
        Ok(good)
    }
}

impl Drop for DeltaStore {
    fn drop(&mut self) {
        // A clean stop makes every edit durable.
        if let Err(e) = self.flush() {
            eprintln!("deltas: final flush failed: {e:#}");
        }
    }
}

/// Applies one edit to a map of chunk deltas.
fn set_in(
    chunks: &mut BTreeMap<(i32, i32, i32), Arc<ChunkDelta>>,
    p: BlockPos,
    id: BlockId,
    palette_above: usize,
) {
    let c = p.chunk();
    let d = chunks
        .entry((c.cx, c.cz, c.cy))
        .or_insert_with(|| Arc::new(ChunkDelta::Sparse(Vec::new())));
    Arc::make_mut(d).set(p.local().index(), id, palette_above);
}

/// CRC-32C (Castagnoli), bit by bit over a small table.
fn crc32c(bytes: &[u8]) -> u32 {
    const fn table() -> [u32; 256] {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0x82f6_3b78 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    }
    const TABLE: [u32; 256] = table();
    let mut c = !0u32;
    for &b in bytes {
        c = TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
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

/// Reads the header of a delta file; returns its generator version and, for a region file,
/// the number of its last journal record (0 before format 2).
fn read_header(path: &Path, magic: &[u8; 8], world_hash: &str) -> anyhow::Result<(u32, u64)> {
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
    let known = if magic == MAGIC { 1..=FORMAT } else { 1..=1 };
    ensure!(known.contains(&format), "unknown delta format {format}");
    let version = r.u32()?;
    let hash = r.str()?;
    if hash != world_hash {
        bail!("deltas made for another world (hash {hash})");
    }
    let mut lsn = 0;
    if magic == MAGIC && format >= 2 {
        let mut b = [0u8; 8];
        f.read_exact(&mut b).context("truncated file")?;
        lsn = u64::from_le_bytes(b);
    }
    Ok((version, lsn))
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
        assert_eq!(segments(&d).len(), 0, "journal emptied");
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

    /// Journal segments of the store in `d`, sorted.
    fn segments(d: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d.join(WAL_DIR))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn automatic_checkpoint_and_truncated_journal() {
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let d = dir("compact");
        let c = DeltaConfig {
            checkpoint_bytes: 300,
            ..DeltaConfig::default()
        };
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, c.clone()).unwrap();
        let mut last = 0;
        for k in 0..13 {
            last = s.set(BlockPos::new(k, 5, 0), stone).unwrap();
        }
        assert_eq!(last, 13);
        assert_eq!(s.flush().unwrap(), 13);
        // Checkpointed at least once: region file written, a single (new) segment left.
        assert!(d.join("r.0.0.bin").exists());
        let seg = segments(&d);
        assert_eq!(seg.len(), 1);
        let len = std::fs::metadata(&seg[0]).unwrap().len();
        // A crash in the middle of the last record.
        let f = OpenOptions::new().write(true).open(&seg[0]).unwrap();
        f.set_len(len - 3).unwrap();
        drop(f);
        drop(s);
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, c.clone()).unwrap();
        assert_eq!(s.get(BlockPos::new(11, 5, 0)), Some(stone));
        assert_eq!(s.get(BlockPos::new(12, 5, 0)), None);
        assert_eq!(s.problems().len(), 1);
        // New records follow the last complete one, numbered on.
        assert_eq!(s.set(BlockPos::new(20, 5, 0), AIR).unwrap(), 13);
        drop(s);
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        assert_eq!(s.get(BlockPos::new(20, 5, 0)), Some(AIR));
        assert_eq!(s.get(BlockPos::new(11, 5, 0)), Some(stone));
        assert!(s.problems().is_empty());
        assert_eq!(s.set(BlockPos::new(21, 5, 0), AIR).unwrap(), 14);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_corrupt_record_before_the_end_refuses_the_store() {
        let b = blocks();
        let d = dir("corrupt");
        let c = DeltaConfig {
            checkpoint_bytes: u64::MAX,
            ..DeltaConfig::default()
        };
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, c.clone()).unwrap();
        for k in 0..4 {
            s.set(BlockPos::new(k, 0, 0), AIR).unwrap();
        }
        drop(s);
        let seg = segments(&d)[0].clone();
        let good = std::fs::read(&seg).unwrap();
        // A flipped bit in the last record.
        let mut bad = good.clone();
        let at = bad.len() - 30;
        bad[at] ^= 0x10;
        std::fs::write(&seg, &bad).unwrap();
        // In the last segment: read as a write cut short, the end is dropped.
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, c.clone()).unwrap();
        assert_eq!(s.problems().len(), 1);
        assert_eq!(s.get(BlockPos::new(0, 0, 0)), Some(AIR));
        assert_eq!(s.get(BlockPos::new(3, 0, 0)), None);
        drop(s);
        // Before the last segment: corruption, the store refuses to open.
        std::fs::write(&seg, &bad).unwrap();
        let next = d.join(WAL_DIR).join("w.0000000000000005.log");
        std::fs::write(&next, &std::fs::read(&seg).unwrap()[..26]).unwrap();
        assert!(DeltaStore::open(&d, 1, "h", &b, c).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn region_journals_of_format_1_are_migrated() {
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let d = dir("migrate");
        std::fs::create_dir_all(&d).unwrap();
        // A region file of format 1 and its journal, as written before the global journal.
        let old = DeltaStore::new(1, "h", &b, DeltaConfig::default());
        let mut chunks = BTreeMap::new();
        set_in(&mut chunks, BlockPos::new(1, 1, 1), stone, 2048);
        let mut bin = old.write_region(&chunks, 0);
        // Format 1: no sequence number after the header.
        let head = old.header(MAGIC).len();
        bin.drain(head..head + 8);
        bin[8..12].copy_from_slice(&1u32.to_le_bytes());
        std::fs::write(d.join("r.0.0.bin"), &bin).unwrap();
        let mut log = old.header(LOG_MAGIC);
        for v in [2i32, 1, 1] {
            log.extend_from_slice(&v.to_le_bytes());
        }
        put_str(&mut log, "sand");
        std::fs::write(d.join("r.0.0.log"), &log).unwrap();
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        assert_eq!(s.get(BlockPos::new(1, 1, 1)), Some(stone));
        assert_eq!(s.get(BlockPos::new(2, 1, 1)), b.id("sand"));
        assert!(!d.join("r.0.0.log").exists(), "migrated");
        assert!(s.problems().is_empty());
        s.set(BlockPos::new(3, 1, 1), AIR).unwrap();
        drop(s);
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        assert_eq!(s.get(BlockPos::new(2, 1, 1)), b.id("sand"));
        assert_eq!(s.get(BlockPos::new(3, 1, 1)), Some(AIR));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn records_already_in_a_region_file_are_not_replayed() {
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let d = dir("lsn");
        let c = DeltaConfig {
            checkpoint_bytes: u64::MAX,
            ..DeltaConfig::default()
        };
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, c.clone()).unwrap();
        s.set(BlockPos::new(0, 0, 0), stone).unwrap();
        s.set(BlockPos::new(0, 0, 0), AIR).unwrap();
        // A crash after the region was written but before the segments were dropped.
        let r = s.region((0, 0));
        s.write_back((0, 0), &mut write(&r)).unwrap();
        drop(r);
        let seg = segments(&d);
        let kept: Vec<Vec<u8>> = seg.iter().map(|p| std::fs::read(p).unwrap()).collect();
        drop(s);
        for (p, bytes) in seg.iter().zip(&kept) {
            std::fs::write(p, bytes).unwrap();
        }
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, c).unwrap();
        assert_eq!(s.get(BlockPos::new(0, 0, 0)), Some(AIR));
        assert_eq!(s.set(BlockPos::new(1, 0, 0), stone).unwrap(), 3);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Edit number `k` of the crash test.
    fn crash_edit(k: u64) -> BlockPos {
        BlockPos::new((k % 64) as i32 * 13, (k / 64) as i32, (k % 7) as i32 * 801)
    }

    /// Child of [`a_killed_process_keeps_every_flushed_edit`]: edits and flushes until killed.
    #[test]
    #[ignore = "run by a_killed_process_keeps_every_flushed_edit"]
    fn crash_child() {
        let Ok(d) = std::env::var("NASCIO_CRASH_DIR") else {
            return;
        };
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let c = DeltaConfig {
            checkpoint_bytes: 20_000,
            max_regions: 3,
            ..DeltaConfig::default()
        };
        let (s, _) = DeltaStore::open(Path::new(&d), 1, "h", &b, c).unwrap();
        let mut out = std::io::stdout();
        for k in 1.. {
            let lsn = s.set(crash_edit(k), stone).unwrap();
            assert_eq!(lsn, k);
            if k % 50 == 0 {
                let done = s.flush().unwrap();
                writeln!(out, "flushed {done}").unwrap();
                out.flush().unwrap();
            }
        }
    }

    #[test]
    fn a_killed_process_keeps_every_flushed_edit() {
        use std::io::BufRead;
        let d = dir("killed");
        std::fs::create_dir_all(&d).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "delta::tests::crash_child",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env("NASCIO_CRASH_DIR", &d)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut flushed = 0u64;
        for line in std::io::BufReader::new(child.stdout.take().unwrap()).lines() {
            if let Some(n) = line.unwrap().strip_prefix("flushed ") {
                flushed = n.parse().unwrap();
                if flushed >= 3_000 {
                    break;
                }
            }
        }
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(flushed >= 3_000, "child stopped early");
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        for k in 1..=flushed {
            assert_eq!(s.get(crash_edit(k)), Some(stone), "edit {k} lost");
        }
        // Numbers go on after the last edit that reached the journal.
        let next = s.set(BlockPos::new(5, 5, 5), AIR).unwrap();
        assert!(next > flushed);
        drop(s);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn leftovers_of_an_interrupted_write_are_ignored() {
        let b = blocks();
        let stone = b.id("granite").unwrap();
        let d = dir("leftover");
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        s.set(BlockPos::new(1, 2, 3), stone).unwrap();
        s.flush().unwrap();
        assert_eq!(s.compact_all().unwrap(), 1);
        drop(s);
        assert!(!d.join("r.0.0.bin.tmp").exists() && !d.join("r.0.0.log").exists());
        // A crash during a durable write leaves only a temporary file, half written.
        std::fs::write(d.join("r.0.0.bin.tmp"), b"NSCDEL").unwrap();
        std::fs::write(d.join("r.0.0.log.tmp"), b"NSC").unwrap();
        let (s, w) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        assert!(w.is_empty());
        assert_eq!(s.get(BlockPos::new(1, 2, 3)), Some(stone));
        // The journal is created whole, then appended to.
        s.set(BlockPos::new(4, 2, 3), AIR).unwrap();
        drop(s);
        let (s, _) = DeltaStore::open(&d, 1, "h", &b, DeltaConfig::default()).unwrap();
        assert_eq!(s.get(BlockPos::new(4, 2, 3)), Some(AIR));
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
            let c = DeltaConfig { palette_above: 16, checkpoint_bytes: 600, max_regions: 2 };
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
