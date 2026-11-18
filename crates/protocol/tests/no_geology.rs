//! Invariant 5 and the author's rule for chantier 4: what the server sends for the horizon and
//! the sky carries no geology (no rock, layer, ore, crust, plate, Moho, deposit or block id)
//! and no seed (a client could generate the world with it).
//!
//! The structs are destructured field by field: adding a field to one of them breaks the
//! build of this test, which forces a look at what the new field reveals.

use nascio_chunk::horizon::{HorizonTile, HorizonVertex, Surface, TILE, TileKey};
use nascio_protocol::horizon::{decode_tile, encode_tile};
use nascio_protocol::sky::{
    FIELDS, Field, GroundColors, HARMONIC_QUANTITIES, ReducedClimate, SkyWorld, WorldSky,
};

/// Words that would reveal what lies under the surface.
const GEOLOGY: [&str; 16] = [
    "rock",
    "geo",
    "ore",
    "crust",
    "plate",
    "moho",
    "deposit",
    "sediment",
    "weathering",
    "block",
    "uplift",
    "hotspot",
    "trap",
    "laterite",
    "placer",
    "seed",
];

/// The only climate quantities sent.
const ALLOWED: [&str; 6] = [
    "cloud_cover_",
    "snow_fraction_",
    "temperature_",
    "humidity_",
    "diurnal_range",
    "cell_height",
];

#[test]
fn sky_fields_are_climate_only() {
    for name in FIELDS {
        assert!(
            ALLOWED.iter().any(|a| name.starts_with(a)),
            "unexpected sky field {name}"
        );
        for word in GEOLOGY {
            assert!(!name.contains(word), "sky field {name} looks like geology");
        }
    }
    for q in HARMONIC_QUANTITIES {
        assert!(ALLOWED.iter().any(|a| a.trim_end_matches('_') == q));
    }
}

#[test]
fn world_sky_holds_settings_climate_and_colours_only() {
    let sky = WorldSky {
        world: SkyWorld {
            clock: Default::default(),
            astro: Default::default(),
            lapse_rate_c_per_m: 0.0065,
            atmosphere: Default::default(),
        },
        climate: ReducedClimate { fields: vec![] },
        ground: GroundColors { rgb: vec![] },
    };
    // Exhaustive patterns: a new field does not compile until it is listed here.
    let WorldSky {
        world,
        climate,
        ground,
    } = sky;
    let SkyWorld {
        clock: _,
        astro: _,
        lapse_rate_c_per_m: _,
        atmosphere: _,
    } = world;
    let ReducedClimate { fields } = climate;
    for f in fields {
        let Field {
            min: _,
            max: _,
            q: _,
        } = f;
    }
    let GroundColors { rgb: _ } = ground;
}

#[test]
fn tile_vertices_hold_the_drawn_surface_only() {
    let v = HorizonVertex {
        height: 12.5,
        water: 0.0,
        surface: Surface::Ground,
        color: [90, 120, 60],
    };
    let HorizonVertex {
        height: _,
        water: _,
        surface,
        color: _,
    } = v;
    // Ground, water or ice: no rock or block.
    assert!(matches!(
        surface,
        Surface::Ground | Surface::Water | Surface::Ice
    ));
    let tile = HorizonTile {
        key: TileKey {
            level: 0,
            i: 1,
            j: 2,
        },
        vertices: vec![v; TILE * TILE],
    };
    let HorizonTile {
        key: _,
        vertices: _,
    } = &tile;
    // The encoding carries nothing beyond the fields: four numbers per vertex at most.
    let bytes = encode_tile(&tile).expect("encodes");
    let raw = zstd::bulk::decompress(&bytes, 1 << 20).expect("decompresses");
    assert!(raw.len() <= 16 + TILE * TILE * (3 + 3 + 1 + 3));
    assert_eq!(decode_tile(&bytes).expect("decodes"), tile);
}
