//! Chantier 3: the spawn sites of a world and the point of each account.

use std::sync::{Arc, OnceLock};

use nascio_client::connect;
use nascio_core::consts::{CHUNK_SIZE, WORLD_SIZE};
use nascio_core::torus;
use nascio_server::accounts::Accounts;
use nascio_server::config::SecretLogin;
use nascio_server::spawn::{Point, Rules, Spawns, draw_point, draw_sites};
use nascio_server::{RunningServer, ServerConfig};
use nascio_voxel::{ColumnSet, GenConfig, Terrain};
use nascio_world::world_io::World;
use nascio_worldgen::config::{Scale, WorldConfig};
use nascio_worldgen::generate::generate_full;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

fn world() -> &'static (World, Arc<Terrain>) {
    static W: OnceLock<(World, Arc<Terrain>)> = OnceLock::new();
    W.get_or_init(|| {
        let world = generate_full(&WorldConfig::new(42, Scale::TwentyFifth))
            .expect("world")
            .world;
        let t = Terrain::from_world(&world, GenConfig::standard().voxel).expect("terrain");
        (world, Arc::new(t))
    })
}

fn rules(world: &World) -> Rules {
    Rules {
        max_elevation_m: 1_000.0,
        polar_s_m: world.config.astro.s_clear,
    }
}

/// The column as the chunk generation builds it: top block below 1 000 m, no water above,
/// outside the polar band.
fn check_column(t: &Terrain, world: &World, p: Point) {
    let col = ColumnSet::new(t, p.x.div_euclid(CHUNK_SIZE), p.z.div_euclid(CHUNK_SIZE));
    let (lx, lz) = (p.x.rem_euclid(CHUNK_SIZE), p.z.rem_euclid(CHUNK_SIZE));
    let top = col.top(lx, lz);
    assert!(top < 1_000, "{p:?}: top {top}");
    assert!(col.water_top(lx, lz) <= top + 1, "{p:?}: under water");
    let s = p.z.min(WORLD_SIZE - p.z);
    assert!(
        f64::from(s) > world.config.astro.s_clear,
        "{p:?}: polar band"
    );
}

#[test]
fn sites_follow_the_rules_and_the_seed() {
    let (world, t) = world();
    let sites = draw_sites(t, 42, 10, rules(world)).unwrap();
    assert_eq!(sites.len(), 10);
    for &p in &sites {
        check_column(t, world, p);
    }
    assert_eq!(draw_sites(t, 42, 10, rules(world)).unwrap(), sites);
    assert_ne!(draw_sites(t, 7, 10, rules(world)).unwrap(), sites);
}

#[test]
fn points_lie_within_the_radius_and_follow_the_rules() {
    let (world, t) = world();
    let sites = draw_sites(t, 42, 10, rules(world)).unwrap();
    let mut rng = ChaCha8Rng::seed_from_u64(1);
    for k in 0..200 {
        let site = sites[k % sites.len()];
        let p = draw_point(t, site, 300.0, rules(world), &mut rng);
        let d = torus::distance((p.x, p.z), (site.x, site.z));
        assert!(d < 300.0, "{p:?} {d} m from {site:?}");
        check_column(t, world, p);
    }
}

#[test]
fn an_account_keeps_its_point() {
    let (world, t) = world();
    let dir = std::env::temp_dir().join(format!("nascio-spawn-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = ServerConfig::default();
    let accounts = Accounts::open(&dir).unwrap();
    accounts.create("alice", "secret-alice", 0).unwrap();
    let spawns = Spawns::new(t.clone(), world, "fp", &cfg, &dir).unwrap();
    assert_eq!(spawns.sites.len(), 10);
    assert!(dir.join("spawn_sites.toml").exists());
    let a = spawns.of(&accounts, "alice").unwrap();
    assert_eq!(spawns.of(&accounts, "alice").unwrap(), a);
    let kept = accounts.spawn("alice").unwrap();
    let site = spawns.sites[kept.site as usize];
    assert!(torus::distance((kept.x, kept.z), (site.x, site.z)) < 300.0);
    // After a restart: the same sites, the same point.
    let accounts = Accounts::open(&dir).unwrap();
    let again = Spawns::new(t.clone(), world, "fp", &cfg, &dir).unwrap();
    assert_eq!(again.sites, spawns.sites);
    assert_eq!(again.of(&accounts, "alice").unwrap(), a);
    // A fixed point for every player.
    let fixed = ServerConfig {
        spawn_xz: Some([1_000.5, 2_000.5]),
        ..ServerConfig::default()
    };
    let f = Spawns::new(t.clone(), world, "fp", &fixed, &dir).unwrap();
    let p = f.of(&accounts, "alice").unwrap();
    assert_eq!((p[0], p[2]), (1_000.5, 2_000.5));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Through the server: a new player gets a point near a site, and the same one at the next
/// login.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_player_comes_back_to_its_point() {
    let (world, _) = world();
    let dir = std::env::temp_dir().join(format!("nascio-spawn-server-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = ServerConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        chunk_radius_m: 32,
        generation_threads: 2,
        account_hash_kib: 256,
        account_hash_passes: 1,
        open_registration: true,
        secret_login: SecretLogin::All,
        ..ServerConfig::default()
    };
    let server = RunningServer::start(world, cfg, &dir).unwrap();
    let c = connect(server.addr, &server.cert_der, "bob", "bob-secret")
        .await
        .unwrap();
    let first = c.hello.spawn;
    c.close();
    let near = server.shared.spawns.sites.iter().any(|s| {
        torus::distance_f64((first[0], first[2]), (f64::from(s.x), f64::from(s.z))) < 301.0
    });
    assert!(near, "{first:?} far from every site");
    // The server may still hold the session for a moment.
    let mut again = None;
    for _ in 0..50 {
        match connect(server.addr, &server.cert_der, "bob", "bob-secret").await {
            Ok(c) => {
                again = Some(c.hello.spawn);
                c.close();
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
        }
    }
    assert_eq!(again, Some(first));
    server.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
