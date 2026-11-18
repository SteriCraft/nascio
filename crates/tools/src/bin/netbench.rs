//! Measurements for the server phase (S0): chunk codecs on a real world, and `fsync` costs.
//!
//! Codecs: the current `Chunk::encode` (v1: palette + `u16` runs in index order), runs with
//! varints in layer order (`x`, `z`, then `y`, the index order) or in column order (`y`
//! fastest), a bit-packed palette, each optionally followed by LZ4 or zstd. Chunks are the
//! ones a server would send: every layer of [`ColumnSet::chunk_range`] at sites spread over
//! the world (`--seed`), split into land and sea columns.
//!
//! ```text
//! cargo run -p nascio-tools --release --bin netbench -- codecs out/m7/world-42
//! cargo run -p nascio-tools --release --bin netbench -- fsync out/netbench
//! cargo run -p nascio-tools --release --bin netbench -- horizon out/server/world-42
//! ```
//!
//! Horizon (chantier 4): compressed size of the horizon tiles a client needs at sites spread
//! over the world, then after a move of 512 m, and of `WorldSky` (reduced climate, far ground).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::ensure;
use clap::{Parser, Subcommand};
use nascio_chunk::horizon::{TileKey, spacing};
use nascio_core::consts::CHUNKS_PER_AXIS;
use nascio_voxel::block::BlockId;
use nascio_voxel::chunk::{N, VOLUME};
use nascio_voxel::{Chunk, ColumnSet, GenConfig, Terrain};
use nascio_worldgen::rng::mix64;
use nascio_worldgen::world_io::World;
use rayon::prelude::*;

#[derive(Parser)]
#[command(name = "netbench", about = "Nascio chunk codec and fsync measurements")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Chunk codecs on the chunks of sites spread over the world.
    Codecs {
        world: PathBuf,
        #[arg(long, default_value_t = 24)]
        sites: u64,
        #[arg(long, default_value_t = 4)]
        side: i32,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Delta journal: edits as fast as one thread can, a flush every `--interval-ms` on
    /// another thread (group commit), checkpoints as configured; in `dir` (emptied).
    Wal {
        dir: PathBuf,
        #[arg(long, default_value_t = 5.0)]
        seconds: f64,
        #[arg(long, default_value_t = 50)]
        interval_ms: u64,
        /// Edits per second asked for (0: as fast as possible).
        #[arg(long, default_value_t = 0)]
        rate: u64,
    },
    /// Horizon tiles and `WorldSky`: what the server sends for the far view.
    Horizon {
        world: PathBuf,
        #[arg(long, default_value_t = 8)]
        sites: u64,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Cost of `fsync` for one record and for groups of records, in `dir`.
    Fsync {
        dir: PathBuf,
        #[arg(long, default_value_t = 200)]
        rounds: usize,
    },
}

/// Index of `(x, y, z)` in column order (`y` fastest, then `x`, then `z`).
fn column_index(i: usize) -> usize {
    let (x, z, y) = (i % N, (i / N) % N, i / (N * N));
    y + N * (x + N * z)
}

fn put_varint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(b: &[u8], at: &mut usize) -> u32 {
    let mut v = 0u32;
    let mut s = 0;
    loop {
        let c = b[*at];
        *at += 1;
        v |= u32::from(c & 0x7f) << s;
        if c < 0x80 {
            return v;
        }
        s += 7;
    }
}

/// Palette in first-use order and the palette index of every block, in `order`.
fn palette(blocks: &[BlockId; VOLUME], column: bool) -> (Vec<BlockId>, Vec<u16>) {
    let mut pal: Vec<BlockId> = Vec::new();
    let mut idx = vec![0u16; VOLUME];
    for (i, &b) in blocks.iter().enumerate() {
        let k = pal.iter().position(|&q| q == b).unwrap_or_else(|| {
            pal.push(b);
            pal.len() - 1
        });
        idx[if column { column_index(i) } else { i }] = k as u16;
    }
    (pal, idx)
}

/// Palette then runs `(index, length - 1)`, all varints.
fn encode_varint(blocks: &[BlockId; VOLUME], column: bool) -> Vec<u8> {
    let (pal, idx) = palette(blocks, column);
    let mut out = Vec::with_capacity(64);
    put_varint(&mut out, pal.len() as u32);
    for &b in &pal {
        put_varint(&mut out, u32::from(b));
    }
    let mut start = 0;
    for i in 1..=VOLUME {
        if i == VOLUME || idx[i] != idx[start] {
            put_varint(&mut out, u32::from(idx[start]));
            put_varint(&mut out, (i - start - 1) as u32);
            start = i;
        }
    }
    out
}

fn decode_varint(b: &[u8], column: bool) -> Box<[BlockId; VOLUME]> {
    let mut at = 0;
    let n = get_varint(b, &mut at) as usize;
    let pal: Vec<BlockId> = (0..n).map(|_| get_varint(b, &mut at) as BlockId).collect();
    let mut out = Box::new([0 as BlockId; VOLUME]);
    let mut i = 0;
    while i < VOLUME {
        let k = get_varint(b, &mut at) as usize;
        let len = get_varint(b, &mut at) as usize + 1;
        for j in i..i + len {
            // Column order back to index order.
            let t = if column {
                let (y, x, z) = (j % N, (j / N) % N, j / (N * N));
                x + N * (z + N * y)
            } else {
                j
            };
            out[t] = pal[k];
        }
        i += len;
    }
    out
}

/// Palette then `ceil(log2(len))` bits per block, in index order.
fn encode_bitpack(blocks: &[BlockId; VOLUME]) -> Vec<u8> {
    let (pal, idx) = palette(blocks, false);
    let mut out = Vec::with_capacity(64);
    put_varint(&mut out, pal.len() as u32);
    for &b in &pal {
        put_varint(&mut out, u32::from(b));
    }
    let bits = usize::BITS - (pal.len() - 1).leading_zeros();
    if bits == 0 {
        return out;
    }
    let (mut acc, mut n) = (0u64, 0u32);
    for &k in &idx {
        acc |= u64::from(k) << n;
        n += bits;
        while n >= 8 {
            out.push(acc as u8);
            acc >>= 8;
            n -= 8;
        }
    }
    if n > 0 {
        out.push(acc as u8);
    }
    out
}

/// Sizes (bytes) of one chunk under every codec, in [`NAMES`] order.
const NAMES: [&str; 13] = [
    "v1 (u16 runs, layer order)",
    "varint runs, layer order",
    "varint runs, column order",
    "bit-packed palette",
    "v1 + lz4",
    "v1 + zstd-3",
    "varint column + lz4",
    "varint column + zstd-1",
    "varint column + zstd-3",
    "bit-packed + lz4",
    "bit-packed + zstd-3",
    "varint layer + lz4",
    "varint layer + zstd-3",
];

fn sizes(c: &Chunk) -> [usize; NAMES.len()] {
    let d = c.to_dense();
    let v1 = c.encode();
    let vl = encode_varint(&d, false);
    let vc = encode_varint(&d, true);
    let bp = encode_bitpack(&d);
    let lz = |b: &[u8]| lz4_flex::compress_prepend_size(b).len();
    let zs = |b: &[u8], l| zstd::bulk::compress(b, l).expect("zstd").len();
    [
        v1.len(),
        vl.len(),
        vc.len(),
        bp.len(),
        lz(&v1),
        zs(&v1, 3),
        lz(&vc),
        zs(&vc, 1),
        zs(&vc, 3),
        lz(&bp),
        zs(&bp, 3),
        lz(&vl),
        zs(&vl, 3),
    ]
}

struct Sample {
    land: bool,
    uniform: bool,
    chunk: Chunk,
}

fn codecs(world: PathBuf, sites: u64, side: i32, seed: u64) -> anyhow::Result<()> {
    let world = World::load(&world)?;
    let cfg = GenConfig::load(None, false)?;
    let terrain = Terrain::from_world(&world, cfg.voxel.clone())?;
    let keys: Vec<(i32, i32)> = (0..sites)
        .flat_map(|s| {
            let h = mix64(seed ^ mix64(s + 1));
            let cx0 = (h % CHUNKS_PER_AXIS as u64) as i32;
            let cz0 = ((h >> 32) % CHUNKS_PER_AXIS as u64) as i32;
            (0..side * side).map(move |i| (cx0 + i % side, cz0 + i / side))
        })
        .collect();
    let underground = terrain.config().underground_chunks;
    let t = Instant::now();
    let samples: Vec<Sample> = keys
        .par_iter()
        .flat_map_iter(|&(cx, cz)| {
            let col = ColumnSet::new(&terrain, cx, cz);
            let land = col.top_range().1 >= 0;
            col.chunk_range(underground)
                .map(|cy| {
                    let chunk = col.padded_chunk(&terrain, cy).inner();
                    Sample {
                        land,
                        uniform: chunk.uniform().is_some(),
                        chunk,
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let land_cols = keys
        .iter()
        .filter(|&&(cx, cz)| ColumnSet::new(&terrain, cx, cz).top_range().1 >= 0)
        .count();
    println!(
        "{} columns ({land_cols} land), {} chunks ({} uniform), generated in {:.1} s",
        keys.len(),
        samples.len(),
        samples.iter().filter(|s| s.uniform).count(),
        t.elapsed().as_secs_f64()
    );
    let all: Vec<[usize; NAMES.len()]> = samples.par_iter().map(|s| sizes(&s.chunk)).collect();
    for (label, filter) in [
        ("land, non-uniform", (true, false)),
        ("sea, non-uniform", (false, false)),
    ] {
        let rows: Vec<&[usize; NAMES.len()]> = samples
            .iter()
            .zip(&all)
            .filter(|(s, _)| (s.land, s.uniform) == filter)
            .map(|(_, r)| r)
            .collect();
        if rows.is_empty() {
            continue;
        }
        println!(
            "\n{label}: {} chunks — bytes per chunk: mean / median / p95 / max",
            rows.len()
        );
        for (k, name) in NAMES.iter().enumerate() {
            let mut v: Vec<usize> = rows.iter().map(|r| r[k]).collect();
            v.sort_unstable();
            let mean = v.iter().sum::<usize>() as f64 / v.len() as f64;
            println!(
                "  {name:<28} {mean:>8.0} {:>8} {:>8} {:>8}",
                v[v.len() / 2],
                v[v.len() * 95 / 100],
                v[v.len() - 1]
            );
        }
    }
    // Bytes per column (every chunk of its range, uniform ones included).
    let cols = keys.len() as f64;
    println!("\nper column (all chunks of the range, {cols} columns): mean bytes");
    for (k, name) in NAMES.iter().enumerate() {
        let tot: usize = all.iter().map(|r| r[k]).sum();
        println!("  {name:<28} {:>8.0}", tot as f64 / cols);
    }

    // A whole column compressed at once (varint layer order, concatenated).
    let mut by_col: std::collections::BTreeMap<(bool, usize), Vec<u8>> = Default::default();
    let mut k = 0;
    for &(cx, cz) in &keys {
        let col = ColumnSet::new(&terrain, cx, cz);
        let n = col.chunk_range(underground).count();
        for s in &samples[k..k + n] {
            by_col
                .entry((s.land, k))
                .or_default()
                .extend(encode_varint(&s.chunk.to_dense(), false));
        }
        k += n;
    }
    for land in [true, false] {
        let sizes: Vec<usize> = by_col
            .iter()
            .filter(|(k, _)| k.0 == land)
            .map(|(_, b)| zstd::bulk::compress(b, 3).expect("zstd").len())
            .collect();
        println!(
            "  column batch, varint + zstd-3, {} columns: {:.0} bytes ({} columns)",
            if land { "land" } else { "sea" },
            sizes.iter().sum::<usize>() as f64 / sizes.len().max(1) as f64,
            sizes.len()
        );
    }

    // The protocol's codec v2 and column batches (what the server will send).
    let mut k = 0;
    let (mut land_b, mut land_n, mut sea_b, mut sea_n) = (0usize, 0usize, 0usize, 0usize);
    let t = Instant::now();
    for &(cx, cz) in &keys {
        let col = ColumnSet::new(&terrain, cx, cz);
        let range = col.chunk_range(underground);
        let n = range.clone().count();
        let c = nascio_protocol::chunk::Column {
            cx,
            cz,
            cy_min: *range.start(),
            version: 0,
            chunks: samples[k..k + n].iter().map(|s| s.chunk.clone()).collect(),
            tint: {
                let mut t = terrain.chunk_tint(cx, cz);
                t.water_depth = col.water_depths();
                t
            },
        };
        let bytes = c.compress()?;
        if samples[k].land {
            (land_b, land_n) = (land_b + bytes.len(), land_n + 1);
        } else {
            (sea_b, sea_n) = (sea_b + bytes.len(), sea_n + 1);
        }
        ensure!(
            nascio_protocol::chunk::Column::decompress(&bytes, u16::MAX - 1)? == c,
            "protocol column round trip"
        );
        k += n;
    }
    println!(
        "  protocol v2 column batch: land {:.0} bytes ({land_n}), sea {:.0} bytes ({sea_n}); {:.2} ms per column (encode + compress + decompress + decode, 1 thread)",
        land_b as f64 / land_n.max(1) as f64,
        sea_b as f64 / sea_n.max(1) as f64,
        t.elapsed().as_secs_f64() * 1e3 / cols
    );

    // Speeds, one thread, on the non-uniform chunks.
    let dense: Vec<Box<[BlockId; VOLUME]>> = samples
        .iter()
        .filter(|s| !s.uniform)
        .map(|s| s.chunk.to_dense())
        .collect();
    let n = dense.len() as f64;
    let time = |f: &dyn Fn(&[BlockId; VOLUME]) -> usize| {
        let t = Instant::now();
        let mut x = 0;
        for d in &dense {
            x += f(d);
        }
        std::hint::black_box(x);
        t.elapsed().as_secs_f64() * 1e6 / n
    };
    println!("\nspeed, 1 thread, non-uniform chunks: µs per chunk");
    let chunks: Vec<Chunk> = dense
        .iter()
        .map(|d| Chunk::from_blocks(d.clone()))
        .collect();
    let t = Instant::now();
    let enc: Vec<Vec<u8>> = chunks.iter().map(Chunk::encode).collect();
    println!(
        "  v1 encode                    {:>8.1}",
        t.elapsed().as_secs_f64() * 1e6 / n
    );
    let t = Instant::now();
    for e in &enc {
        std::hint::black_box(Chunk::decode(e)?);
    }
    println!(
        "  v1 decode                    {:>8.1}",
        t.elapsed().as_secs_f64() * 1e6 / n
    );
    println!(
        "  protocol v2 encode           {:>8.1}",
        time(&|d| {
            let mut b = Vec::new();
            nascio_protocol::chunk::encode(&Chunk::from_blocks(Box::new(*d)), &mut b);
            b.len()
        })
    );
    println!(
        "  varint layer encode          {:>8.1}",
        time(&|d| encode_varint(d, false).len())
    );
    println!(
        "  varint layer + zstd-3 enc.   {:>8.1}",
        time(&|d| zstd::bulk::compress(&encode_varint(d, false), 3)
            .expect("zstd")
            .len())
    );
    let vl: Vec<Vec<u8>> = dense.iter().map(|d| encode_varint(d, false)).collect();
    let t = Instant::now();
    for e in &vl {
        std::hint::black_box(decode_varint(e, false));
    }
    println!(
        "  varint layer decode          {:>8.1}",
        t.elapsed().as_secs_f64() * 1e6 / n
    );
    println!(
        "  varint column encode         {:>8.1}",
        time(&|d| encode_varint(d, true).len())
    );
    let vc: Vec<Vec<u8>> = dense.iter().map(|d| encode_varint(d, true)).collect();
    for (d, e) in dense.iter().zip(&vc).take(50) {
        ensure!(*decode_varint(e, true) == **d, "varint column round trip");
    }
    let t = Instant::now();
    for e in &vc {
        std::hint::black_box(decode_varint(e, true));
    }
    println!(
        "  varint column decode         {:>8.1}",
        t.elapsed().as_secs_f64() * 1e6 / n
    );
    println!(
        "  varint column + zstd-3 enc.  {:>8.1}",
        time(&|d| zstd::bulk::compress(&encode_varint(d, true), 3)
            .expect("zstd")
            .len())
    );
    let z: Vec<Vec<u8>> = vc
        .iter()
        .map(|e| zstd::bulk::compress(e, 3))
        .collect::<Result<_, _>>()?;
    let t = Instant::now();
    for (e, raw) in z.iter().zip(&vc) {
        std::hint::black_box(zstd::bulk::decompress(e, raw.len())?);
    }
    println!(
        "  zstd-3 decompress only       {:>8.1}",
        t.elapsed().as_secs_f64() * 1e6 / n
    );
    println!(
        "  varint column + lz4 enc.     {:>8.1}",
        time(&|d| lz4_flex::compress_prepend_size(&encode_varint(d, true)).len())
    );
    println!(
        "  bit-packed + zstd-3 enc.     {:>8.1}",
        time(&|d| zstd::bulk::compress(&encode_bitpack(d), 3)
            .expect("zstd")
            .len())
    );
    Ok(())
}

fn fsync(dir: PathBuf, rounds: usize) -> anyhow::Result<()> {
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("fsync-bench.log");
    let _ = std::fs::remove_file(&path);
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    // A typical edit record: position, block, sequence number, CRC.
    let rec = [0x5au8; 24];
    println!("{}", path.display());
    for group in [1usize, 10, 100, 1000] {
        let mut lat: Vec<Duration> = Vec::with_capacity(rounds);
        let t = Instant::now();
        for _ in 0..rounds {
            let t0 = Instant::now();
            for _ in 0..group {
                f.write_all(&rec)?;
            }
            f.sync_data()?;
            lat.push(t0.elapsed());
        }
        let wall = t.elapsed().as_secs_f64();
        lat.sort_unstable();
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        println!(
            "group of {group:>4} records: fsync median {:.2} ms, p99 {:.2} ms, max {:.2} ms; {:.0} records/s",
            ms(lat[rounds / 2]),
            ms(lat[rounds * 99 / 100]),
            ms(lat[rounds - 1]),
            (group * rounds) as f64 / wall
        );
    }
    let t = Instant::now();
    for _ in 0..100_000 {
        f.write_all(&rec)?;
    }
    println!(
        "no fsync: {:.0} records/s (written to the OS cache)",
        100_000.0 / t.elapsed().as_secs_f64()
    );
    drop(f);
    std::fs::remove_file(&path)?;
    Ok(())
}

fn wal(dir: PathBuf, seconds: f64, interval_ms: u64, rate: u64) -> anyhow::Result<()> {
    use nascio_core::coords::BlockPos;
    use nascio_voxel::{DeltaConfig, DeltaStore};
    use std::sync::atomic::{AtomicBool, Ordering};
    let _ = std::fs::remove_dir_all(&dir);
    let blocks = nascio_voxel::block::Blocks::builtin();
    let stone = blocks.id("granite").expect("granite");
    let (store, _) = DeltaStore::open(&dir, 0, "netbench", &blocks, DeltaConfig::default())?;
    let stop = AtomicBool::new(false);
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let pct = |v: &mut Vec<Duration>, q: f64| {
        v.sort_unstable();
        v.get(((v.len() as f64 - 1.0) * q) as usize)
            .copied()
            .unwrap_or_default()
    };
    let (mut sets, mut flushes, edits) = std::thread::scope(|sc| {
        let flusher = sc.spawn(|| {
            let mut lat = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(interval_ms));
                let t = Instant::now();
                store.flush().expect("flush");
                lat.push(t.elapsed());
            }
            lat
        });
        let start = Instant::now();
        let mut lat = Vec::new();
        let mut k: u64 = 0;
        // A battle: every edit in a 200 m square (one or two regions).
        while start.elapsed().as_secs_f64() < seconds {
            if rate > 0 {
                let due = Duration::from_secs_f64(k as f64 / rate as f64);
                if let Some(wait) = due.checked_sub(start.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            let h = mix64(k);
            let p = BlockPos::new(
                (h % 200) as i32,
                ((h >> 16) % 64) as i32,
                ((h >> 32) % 200) as i32,
            );
            let t = Instant::now();
            store.set(p, stone).expect("set");
            lat.push(t.elapsed());
            k += 1;
        }
        stop.store(true, Ordering::Relaxed);
        (lat, flusher.join().expect("flusher"), k)
    });
    println!(
        "{edits} edits in {seconds} s: {:.0} edits/s; flush every {interval_ms} ms",
        edits as f64 / seconds
    );
    println!(
        "  set: median {:.4} ms, p99 {:.4} ms, max {:.2} ms (max includes checkpoints)",
        ms(pct(&mut sets, 0.5)),
        ms(pct(&mut sets, 0.99)),
        ms(pct(&mut sets, 1.0))
    );
    println!(
        "  flush ({}): median {:.2} ms, p99 {:.2} ms, max {:.2} ms",
        flushes.len(),
        ms(pct(&mut flushes, 0.5)),
        ms(pct(&mut flushes, 0.99)),
        ms(pct(&mut flushes, 1.0))
    );
    let t = Instant::now();
    let n = store.checkpoint()?;
    println!(
        "  final checkpoint: {n} regions in {:.1} ms",
        ms(t.elapsed())
    );
    drop(store);
    let t = Instant::now();
    let (store, _) = DeltaStore::open(&dir, 0, "netbench", &blocks, DeltaConfig::default())?;
    println!("  reopen: {:.1} ms", ms(t.elapsed()));
    drop(store);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

/// Tiles of the client's horizon around `center` (the viewer's layout: 5 levels of 192 cells
/// from 16 m, centre a multiple of 512 m; see `crates/viewer/src/horizon.rs`).
fn horizon_tiles(center: (i64, i64)) -> std::collections::BTreeSet<TileKey> {
    let mut set = std::collections::BTreeSet::new();
    for level in 0..5u8 {
        let s = spacing(level) as i64;
        let half = 96 * s;
        let (oi, oj) = ((center.0 - half) / s, (center.1 - half) / s);
        let (a, _) = TileKey::containing(level, oi - 1, oj - 1);
        let (b, _) = TileKey::containing(level, oi + 193, oj + 193);
        for j in a.j..=b.j {
            for i in a.i..=b.i {
                set.insert(TileKey { level, i, j });
            }
        }
    }
    set
}

fn horizon(dir: PathBuf, sites: u64, seed: u64) -> anyhow::Result<()> {
    let world = World::load(&dir)?;
    let terrain = Terrain::from_world(&world, GenConfig::standard().voxel)?;
    println!(
        "world {} (generator {})",
        dir.display(),
        world.config.generator_version
    );
    let t = Instant::now();
    let sky = nascio_server::horizon::world_sky(&world, &terrain)?;
    let sky_s = t.elapsed().as_secs_f64();
    let climate = nascio_server::horizon::reduced_climate(&world)?;
    let climate_z = zstd::bulk::compress(&postcard::to_stdvec(&climate)?, 9)?;
    println!(
        "WorldSky: {} KB compressed ({:.1} s to build); reduced climate alone {} KB, far ground about {} KB",
        sky.len() >> 10,
        sky_s,
        climate_z.len() >> 10,
        (sky.len() - climate_z.len().min(sky.len())) >> 10
    );
    let snap = 512i64;
    let (mut all_tiles, mut all_bytes, mut all_raw) = (0usize, 0usize, 0usize);
    let (mut move_tiles, mut move_bytes) = (0usize, 0usize);
    let mut build_s = 0.0;
    for k in 0..sites {
        let h = mix64(seed ^ k.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let size = nascio_core::consts::WORLD_SIZE as i64;
        let c = (
            ((h % size as u64) as i64 / snap) * snap,
            (((h >> 32) % size as u64) as i64 / snap) * snap,
        );
        let here = horizon_tiles(c);
        let there = horizon_tiles((c.0 + snap, c.1));
        let encode = |key: TileKey| -> anyhow::Result<(usize, usize)> {
            let tile = nascio_voxel::horizon::horizon_tile(&terrain, key);
            let bytes = nascio_protocol::horizon::encode_tile(&tile)?;
            let raw = zstd::bulk::decompress(&bytes, 1 << 20)?.len();
            Ok((bytes.len(), raw))
        };
        let t = Instant::now();
        let sizes: Vec<(usize, usize)> = here
            .par_iter()
            .map(|&key| encode(key))
            .collect::<anyhow::Result<_>>()?;
        build_s += t.elapsed().as_secs_f64();
        let bytes: usize = sizes.iter().map(|s| s.0).sum();
        let raw: usize = sizes.iter().map(|s| s.1).sum();
        let new: Vec<TileKey> = there.difference(&here).copied().collect();
        let moved: usize = new
            .par_iter()
            .map(|&key| encode(key).map(|s| s.0))
            .collect::<anyhow::Result<Vec<_>>>()?
            .iter()
            .sum();
        println!(
            "site {k} ({:>6}, {:>6}): {} tiles, {} KB ({} KB before zstd); +512 m east: {} tiles, {} KB",
            c.0,
            c.1,
            here.len(),
            bytes >> 10,
            raw >> 10,
            new.len(),
            moved >> 10
        );
        all_tiles += here.len();
        all_bytes += bytes;
        all_raw += raw;
        move_tiles += new.len();
        move_bytes += moved;
    }
    let n = sites.max(1) as usize;
    println!(
        "mean: horizon {} tiles, {} KB ({:.0} B per tile, {} KB before zstd), built in {:.2} s on {} threads; move of 512 m: {} tiles, {} KB",
        all_tiles / n,
        (all_bytes / n) >> 10,
        all_bytes as f64 / all_tiles.max(1) as f64,
        (all_raw / n) >> 10,
        build_s / n as f64,
        rayon::current_num_threads(),
        move_tiles / n,
        (move_bytes / n) >> 10
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Codecs {
            world,
            sites,
            side,
            seed,
        } => codecs(world, sites, side, seed),
        Cmd::Fsync { dir, rounds } => fsync(dir, rounds),
        Cmd::Horizon { world, sites, seed } => horizon(world, sites, seed),
        Cmd::Wal {
            dir,
            seconds,
            interval_ms,
            rate,
        } => wal(dir, seconds, interval_ms, rate),
    }
}
