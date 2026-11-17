//! Climate, biomes and glaciers through the chunk path (spec §4.3, §4.5, §5; audit M11-1):
//! zonal order of the biomes, their continuity across the seam, and the glaciers, snow and sea
//! ice of the generated blocks against the column climate. Run on the 1/5 world in every
//! `cargo test`; the same checks run at full scale on the reference world `out/m7/world-42`
//! with `cargo test -p nascio-voxel --release --test climate_path -- --ignored`.

use std::path::Path;
use std::sync::OnceLock;

use nascio_core::consts::{CHUNK_SIZE, CHUNKS_PER_AXIS, WORLD_SIZE};
use nascio_core::geo::seam_distance;
use nascio_voxel::{ColumnSet, Terrain, VoxelConfig};
use nascio_world::biomes::code;
use nascio_world::world_io::{LayerData, World};
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;

fn fifth() -> &'static (World, Terrain) {
    static T: OnceLock<(World, Terrain)> = OnceLock::new();
    T.get_or_init(|| {
        let g = generate_full(&WorldConfig::new(42, Scale::Fifth)).expect("generation");
        let t = Terrain::from_world(&g.world, VoxelConfig::default()).expect("terrain");
        (g.world, t)
    })
}

/// Biome grid of a world, with its width.
fn biomes(w: &World) -> (&[u8], usize) {
    let l = w.layer("biome").expect("biome layer");
    match &l.data {
        LayerData::U8(v) => (v, l.width),
        _ => panic!("biome layer is not u8"),
    }
}

/// Share of `pred` among the land cells (not ocean) of the rows whose centre lies within `band`
/// m of seam distance.
fn land_share(w: &World, band: std::ops::Range<f64>, pred: impl Fn(u8) -> bool) -> f64 {
    let (b, res) = biomes(w);
    let cell = f64::from(WORLD_SIZE) / res as f64;
    let (mut n, mut k) = (0usize, 0usize);
    for j in 0..res {
        if !band.contains(&seam_distance((j as f64 + 0.5) * cell)) {
            continue;
        }
        for &c in &b[j * res..(j + 1) * res] {
            if c != code::OCEAN && c != code::LAKE {
                n += 1;
                k += usize::from(pred(c));
            }
        }
    }
    assert!(n > 0, "no land in band {band:?}");
    k as f64 / n as f64
}

fn biomes_follow_latitude(w: &World) {
    let ice = |c| c == code::ICE;
    let tropical = |c| matches!(c, code::RAINFOREST | code::SAVANNA | code::MANGROVE);
    // Polar band (§4.3: permanent snow on land) under ice, none in the tropics.
    let polar = land_share(w, 0.0..2_500.0, ice);
    assert!(polar > 0.9, "ice on {polar:.3} of the polar land");
    assert!(land_share(w, 45_000.0..50_000.0, ice) < 0.02);
    // Tropical climates at the equator only (the 1/5 and full worlds: 0.85 to 0.87).
    let eq = land_share(w, 45_000.0..50_000.0, tropical);
    assert!(eq > 0.6, "tropical on {eq:.3} of the equatorial land");
    assert_eq!(land_share(w, 0.0..12_500.0, tropical), 0.0);
}

/// Both sides of the seam: the biome mix of the rows just north and just south of it (seed 42:
/// at most 0.08 apart, for any biome).
fn biomes_continuous_across_the_seam(w: &World) {
    let (b, res) = biomes(w);
    let rows = (res / 40).max(2);
    let mix = |js: std::ops::Range<usize>| {
        let mut h = [0.0f64; code::COUNT];
        for &c in &b[js.start * res..js.end * res] {
            h[usize::from(c)] += 1.0;
        }
        h.map(|v| v / (js.len() * res) as f64)
    };
    let (north, south) = (mix(0..rows), mix(res - rows..res));
    for c in 0..code::COUNT {
        let d = (north[c] - south[c]).abs();
        assert!(
            d < 0.15,
            "{}: {d:.3} apart across the seam",
            nascio_world::biomes::NAMES[c]
        );
    }
}

/// Columns on a lattice of chunks, `(cx, cz)`, with the chunk rows on both sides of the seam.
fn sample_chunks() -> Vec<(i32, i32)> {
    let mut rows: Vec<i32> = (0..CHUNKS_PER_AXIS).step_by(97).collect();
    rows.extend([
        1,
        2,
        CHUNKS_PER_AXIS - 3,
        CHUNKS_PER_AXIS - 2,
        CHUNKS_PER_AXIS - 1,
    ]);
    (0..CHUNKS_PER_AXIS)
        .step_by(61)
        .flat_map(|cx| rows.iter().map(move |&cz| (cx, cz)))
        .collect()
}

/// The blocks generated at the surface agree with the column climate: glaciers where the
/// warmest month is below `glacier_warm_c` and none far above it, thin snow only where cold
/// enough, ice on the water exactly where it is frozen.
fn glaciers_and_snow_follow_the_climate(t: &Terrain) {
    let ids = t.blocks().ids;
    let cfg = t.config();
    let (ice, snow) = (ids.ice.expect("ice"), ids.snow.expect("snow"));
    let layer = ids.snow_layer.expect("snow layer");
    let (mut glaciers, mut bare, mut snowy, mut frozen, mut open) = (0, 0, 0, 0, 0);
    for (cx, cz) in sample_chunks() {
        let col = ColumnSet::new(t, cx, cz);
        for (x, z) in [(5, 7), (19, 26), (30, 1)] {
            let (top, wt) = (col.top(x, z), col.water_top(x, z));
            let geo = col.geo(x, z);
            let at = (cx * CHUNK_SIZE + x, cz * CHUNK_SIZE + z);
            let surface = col.block(t, x, top, z);
            let tw = t.t_warm(&geo.climate, top);
            let glacier = surface == ice || surface == snow;
            if wt > top {
                // Water: frozen on top (sea ice, lake and river ice) exactly when its warmest
                // month, dithered, is below the threshold.
                let frozen_here = col.block(t, x, wt - 1, z) == ice;
                assert_eq!(frozen_here, t.water_frozen(&geo.climate), "{at:?}");
                if frozen_here {
                    frozen += 1;
                } else {
                    open += 1;
                }
                continue;
            }
            if top > cfg.beach_high_m && col.slope(x, z) <= cfg.rock_slope {
                if tw < cfg.glacier_warm_c {
                    assert!(glacier, "{at:?}: warmest month {tw:.1} °C, no glacier");
                    glaciers += 1;
                } else if tw > cfg.glacier_warm_c + 8.0 {
                    // Beyond the margin where the ice biome is looked up (`ICE_BIOME_MARGIN_C`).
                    assert!(!glacier, "{at:?}: warmest month {tw:.1} °C, glacier");
                    bare += 1;
                }
            }
            if col.block(t, x, top + 1, z) == layer {
                assert!(!glacier, "{at:?}: snow layer on a glacier");
                assert!(
                    t.t_warm(&geo.climate, top + 1) < cfg.snow_layer_warm_c,
                    "{at:?}"
                );
                snowy += 1;
            }
        }
    }
    // Every case is met on both worlds of seed 42.
    println!("glaciers {glaciers}, bare {bare}, snow layer {snowy}, frozen {frozen}, open {open}");
    for (what, n) in [("glacier", glaciers), ("bare", bare), ("snow layer", snowy)] {
        assert!(n > 0, "no {what} column sampled");
    }
    assert!(frozen > 0 && open > 0, "frozen {frozen}, open {open}");
}

#[test]
fn fifth_scale_climate_path() {
    let (w, t) = fifth();
    biomes_follow_latitude(w);
    biomes_continuous_across_the_seam(w);
    glaciers_and_snow_follow_the_climate(t);
}

#[test]
#[ignore = "full scale: needs out/m7/world-42 (generator version 17), run in release"]
fn full_scale_climate_path() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/m7/world-42");
    let w = World::load(&dir).expect("reference world out/m7/world-42");
    let t = Terrain::from_world(&w, VoxelConfig::default()).expect("terrain");
    biomes_follow_latitude(&w);
    biomes_continuous_across_the_seam(&w);
    glaciers_and_snow_follow_the_climate(&t);
}
