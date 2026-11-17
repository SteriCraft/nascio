//! Reference hashes of generated chunks (audit M7, E5 and M11): the same bits on every
//! platform (the CI runs this on Linux and Windows) and for a given `GENERATOR_VERSION`.
//!
//! A change of generated output must bump `GENERATOR_VERSION`; the new hashes are then
//! printed by the failing assertion.

use std::sync::OnceLock;

use nascio_core::consts::CHUNKS_PER_AXIS;
use nascio_core::coords::ChunkPos;
use nascio_voxel::{ColumnSet, Terrain, VoxelConfig, generate_chunk};
use nascio_worldgen::config::{GENERATOR_VERSION, Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;
use nascio_worldgen::rng::mix64;
use nascio_worldgen::world_io::World;

fn world() -> &'static (World, Terrain) {
    static W: OnceLock<(World, Terrain)> = OnceLock::new();
    W.get_or_init(|| {
        let g = generate_full(&WorldConfig::new(42, Scale::TwentyFifth)).expect("generation");
        let t = Terrain::from_world(&g.world, VoxelConfig::default()).expect("terrain");
        (g.world, t)
    })
}

/// Seam columns (both axes, the corner), then columns drawn by `mix64`.
fn columns() -> Vec<(i32, i32)> {
    let last = CHUNKS_PER_AXIS - 1;
    let mut c = vec![
        (0, 0),
        (last, 0),
        (0, last),
        (last, last),
        (last, 1_562),
        (1_562, last),
    ];
    c.extend((0..6u64).map(|s| {
        let h = mix64(0x60_1d ^ mix64(s + 1));
        (
            (h % CHUNKS_PER_AXIS as u64) as i32,
            ((h >> 32) % CHUNKS_PER_AXIS as u64) as i32,
        )
    }));
    c
}

/// The chunk holding the surface at the column centre, and one 96 m below it.
fn chunks() -> Vec<ChunkPos> {
    let (_, t) = world();
    columns()
        .into_iter()
        .flat_map(|(cx, cz)| {
            let cy = ColumnSet::new(t, cx, cz).top(16, 16).div_euclid(32);
            [ChunkPos::new(cx, cy, cz), ChunkPos::new(cx, cy - 3, cz)]
        })
        .collect()
}

fn chunk_hash(t: &Terrain, p: ChunkPos) -> String {
    let c = generate_chunk(t, p);
    let bytes: Vec<u8> = c.blocks().iter().flat_map(|b| b.to_le_bytes()).collect();
    blake3::hash(&bytes).to_hex()[..16].to_string()
}

const GOLDEN_WORLD: &str = "d81b05353fe038e1438a65aae795aec57148711c07355ef69f04c17a03ded883";
const GOLDEN_CHUNKS: [&str; 24] = [
    "df4d14965dd2f0f5",
    "2ca7167ea364b601",
    "e6023b1f1af59076",
    "a0a15357a690ef5b",
    "449f62d63ed71e46",
    "9ad3071cc973b6fa",
    "e90ec17f86136c22",
    "3e27c4fa72d20f0d",
    "a33a47d68298c032",
    "c4eac0f06bd23d44",
    "1b18de932e3af686",
    "334542625eb89145",
    "9b0cce629bcae363",
    "0445a6ba4cd05489",
    "b4e98c2f53c9cea5",
    "3026edee840c118f",
    "54f85aa1c4bcb01f",
    "1fd473f891c0ba41",
    "8dcece9245b88d79",
    "7e4081ee18fb19b4",
    "d1cc3fd066723c94",
    "7e4081ee18fb19b4",
    "b72774b6f5a9b0fd",
    "87f16344847bce9b",
];

#[test]
fn world_hash_is_the_reference() {
    let (w, _) = world();
    assert_eq!(
        (GENERATOR_VERSION, w.hash().as_str()),
        (15, GOLDEN_WORLD),
        "world of seed 42 at 1/25 changed"
    );
}

#[test]
fn chunk_hashes_are_the_reference() {
    let (_, t) = world();
    let got: Vec<String> = chunks().into_iter().map(|p| chunk_hash(t, p)).collect();
    let listing: Vec<String> = chunks()
        .iter()
        .zip(&got)
        .map(|(p, h)| format!("({}, {}, {}) {h}", p.cx, p.cy, p.cz))
        .collect();
    assert_eq!(
        (GENERATOR_VERSION, got.as_slice()),
        (15, GOLDEN_CHUNKS.map(String::from).as_slice()),
        "chunks changed:\n{}",
        listing.join("\n")
    );
}

/// A chunk one world period away (both axes) is the same chunk.
#[test]
fn chunk_one_period_away_is_identical() {
    let (_, t) = world();
    for (cx, cz) in columns().into_iter().take(6) {
        let cy = ColumnSet::new(t, cx, cz).top(0, 0).div_euclid(32);
        let a = generate_chunk(t, ChunkPos::new(cx, cy, cz));
        let b = ColumnSet::new(t, cx + CHUNKS_PER_AXIS, cz - CHUNKS_PER_AXIS)
            .padded_chunk(t, cy)
            .inner();
        assert!(a.blocks() == b.blocks(), "({cx}, {cz})");
    }
}

/// Exact bits of the surface height on a 64 × 64 grid across the world (seams included):
/// far more sensitive than the chunks, which round heights to the block.
fn height_bits_hash(t: &Terrain) -> String {
    let mut h = blake3::Hasher::new();
    for j in 0..64 {
        for i in 0..64 {
            let (x, z) = (f64::from(i) * 1_562.5 + 0.25, f64::from(j) * 1_562.5 + 0.25);
            h.update(&t.height(x, z).to_bits().to_le_bytes());
        }
    }
    h.finalize().to_hex()[..16].to_string()
}

#[test]
fn height_bits_are_the_reference() {
    let (_, t) = world();
    assert_eq!(
        (GENERATOR_VERSION, height_bits_hash(t).as_str()),
        (15, "e7864faa4a66e8a0"),
        "surface heights changed (bits)"
    );
}
