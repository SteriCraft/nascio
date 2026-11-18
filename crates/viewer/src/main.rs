//! Nascio client and development viewer.
//!
//! It opens on a menu ("Se connecter", "Paramètres", "Quitter"); once connected to a server,
//! the 3D view shows the columns it sends, with the chat (Enter), a debug panel (F3) and a menu (Esc). The
//! world map lives in the server's window. The account and the display settings are kept in
//! `client-settings.toml` next to the executable ([`settings`]); the options of the command
//! line override them for the session. `--fly` flies over a local world without a server (development: chunks
//! generated here, benchmarks, captures).
//!
//! ```text
//! cargo run -p nascio-viewer --release -- out/server/world-42
//! cargo run -p nascio-viewer --release -- out/server/world-42 --fly --pos 21000,1500
//! ```

mod bench;
mod chat;
mod curvature;
mod fly;
mod generating;
mod god;
mod horizon;
mod menu;
mod pause;
mod perf;
mod remote;
mod settings;
mod sky;
mod stars;
mod tiles;

use std::path::PathBuf;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::view::window::screenshot::{Screenshot, save_to_disk};
use bevy::window::PresentMode;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use clap::Parser;
use nascio_core::consts::WORLD_SIZE_F64;
use nascio_core::coords::{BlockPos, WorldPos};
use nascio_voxel::{DEV_OVERRIDE_MARK, DeltaStore, GenConfig, Terrain};
use nascio_worldgen::world_io::World;

use fly::{FlyCam, FlySettings, RenderOrigin};

#[derive(Parser, Debug)]
#[command(about = "Nascio world viewer")]
struct Args {
    /// World directory (from `nascio-worldgen generate`). The server's world: the sky and the
    /// horizon still come from it (until the server sends them).
    #[arg(default_value = "out/server/world-42")]
    world: PathBuf,
    /// Fly over the local world without a server (development).
    #[arg(long)]
    fly: bool,
    /// Camera position `x,z` (height: 60 m above ground) or `x,y,z` (may be underground).
    #[arg(long, value_delimiter = ',')]
    pos: Option<Vec<f64>>,
    /// Heading from north towards east, degrees.
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    yaw: f64,
    /// Pitch, degrees (negative looks down).
    #[arg(long, default_value_t = -15.0, allow_hyphen_values = true)]
    pitch: f64,
    /// Camera speed, m/s.
    #[arg(long, default_value_t = 40.0)]
    speed: f64,
    /// Chunk view distance, m (default: the saved setting, else 1 024 m).
    #[arg(long)]
    view_distance: Option<f32>,
    /// Fixed local solar hour for lighting (default: real time).
    #[arg(long)]
    hour: Option<f64>,
    /// Day of the year (0..365) for the 3D sky (default: real time).
    #[arg(long)]
    day: Option<f64>,
    /// Disable the sun shadows.
    #[arg(long)]
    no_shadows: bool,
    /// Disable the horizon.
    #[arg(long)]
    no_horizon: bool,
    /// Force the weather cloud cover everywhere, 0 to 1 (captures; default: the climate).
    #[arg(long)]
    clouds: Option<f64>,
    /// Capture the window once the scene is loaded, then quit.
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// TOML file with a `[voxel]` section overriding chunk generation parameters (needs
    /// `--dev-override`: it changes the world's chunks).
    #[arg(long)]
    voxel_config: Option<PathBuf>,
    /// Allow `--voxel-config`. Deltas are then kept in memory and every output (window
    /// title, screenshot, benchmark files) is marked `DEV-OVERRIDE`.
    #[arg(long)]
    dev_override: bool,
    /// TOML file with a `[render]` section (display curvature radius…).
    #[arg(long)]
    render_config: Option<PathBuf>,
    /// Display curvature radius `R_vis`, m (render only; 0: flat). Overrides `--render-config`.
    #[arg(long)]
    curvature_radius: Option<f64>,
    /// Borderless fullscreen.
    #[arg(long)]
    fullscreen: bool,
    /// Disable vsync (measure the real frame cost).
    #[arg(long)]
    no_vsync: bool,
    /// Connect at once to this server (`ip`, `ip:port`), without the menu. Sky and horizon
    /// still come from the local world, which must be the server's.
    #[arg(long)]
    connect: Option<String>,
    /// Server certificate (DER). Default: the one the server gives with its answer to the
    /// scan probe, pinned at the first connection to an address (`--known-servers`).
    #[arg(long)]
    cert: Option<PathBuf>,
    /// Certificates pinned by server address.
    #[arg(long, default_value = "out/client/known_servers.toml")]
    known_servers: PathBuf,
    /// Account name (created on first use if the server allows it). Default: the saved one.
    #[arg(long)]
    name: Option<String>,
    /// Account secret (at least 8 characters). Default: the saved one.
    #[arg(long)]
    secret: Option<String>,
    /// Log in with this token from the site (`site token … --name <name>`) instead of the
    /// secret, which servers keep for administrators.
    #[arg(long)]
    token: Option<String>,
    /// Run a scripted benchmark path (TOML, see `bench/default.toml`), then quit.
    /// Implies the 3D mode, 1920x1080, no vsync and the file's settings.
    #[arg(long)]
    bench: Option<PathBuf>,
    /// Benchmark output: one CSV line per frame, plus a `.summary.txt` next to it.
    #[arg(long, default_value = "out/bench.csv")]
    bench_out: PathBuf,
    /// Generate world SEED into the world directory with a progress window, then quit.
    #[arg(long, value_name = "SEED")]
    generate: Option<u64>,
    /// With `--generate`: resolution divisor (1, 5 or 25).
    #[arg(long, default_value_t = 1)]
    scale: u32,
    /// With `--generate`: TOML file overriding generation parameters.
    #[arg(long)]
    world_config: Option<PathBuf>,
    /// With `--generate`: deposit catalogue (default: config/deposits.toml, else built in).
    #[arg(long)]
    deposits: Option<PathBuf>,
}

/// The menu (and its connection dialog), then the 3D view.
#[derive(States, Default, Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub enum Mode {
    #[default]
    Menu,
    Fly,
}

fn main() {
    let args = Args::parse();
    if let Err(e) = run(args) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(mut args: Args) -> anyhow::Result<()> {
    if let Some(seed) = args.generate {
        return generating::run(
            seed,
            args.scale,
            args.world_config.clone(),
            args.deposits.clone(),
            args.world.clone(),
            args.screenshot.clone(),
        );
    }
    let bench_file = args
        .bench
        .as_deref()
        .map(bench::BenchFile::load)
        .transpose()?;
    if let Some(b) = &bench_file {
        args.fly = true;
        args.no_vsync = true;
        args.no_horizon = false;
        args.view_distance = Some(b.view_distance_m.unwrap_or(fly::VIEW_MAX));
        args.hour = Some(b.hour.unwrap_or(10.0));
        if b.curvature_radius_m.is_some() {
            args.curvature_radius = b.curvature_radius_m;
        }
        let w = &b.waypoint[0];
        args.pos = Some(vec![w.x, w.z]);
    }
    let world = World::load(&args.world).map_err(|e| {
        anyhow::anyhow!(
            "cannot load world from {}: {e:#}\ngenerate one first: cargo run -p nascio-worldgen --release -- generate --seed 42 --scale 1",
            args.world.display()
        )
    })?;
    let gen_cfg = GenConfig::load(args.voxel_config.as_deref(), args.dev_override)?;
    if let Some(p) = args.screenshot.take() {
        args.screenshot = Some(gen_cfg.mark_path(p));
    }
    args.bench_out = gen_cfg.mark_path(args.bench_out.clone());
    let fingerprint = gen_cfg.fingerprint(&world);
    let mut render_cfg = match &args.render_config {
        Some(p) => curvature::RenderConfig::from_toml(&std::fs::read_to_string(p)?)?,
        None => curvature::RenderConfig::default(),
    };
    if let Some(r) = args.curvature_radius {
        render_cfg.curvature_radius_m = r;
    }
    let delta_cfg = nascio_voxel::DeltaConfig {
        palette_above: gen_cfg.voxel.delta_palette_above,
        ..Default::default()
    };
    let terrain = Arc::new(Terrain::from_world(&world, gen_cfg.voxel.clone())?);
    // Player modifications (god mode), `world-<seed>/deltas/`. Deltas of another world are
    // left untouched: edits then stay in memory, as under `--dev-override` (other chunks).
    let opened = if args.connect.is_some() {
        Err(anyhow::anyhow!("network mode: the server holds the edits"))
    } else if gen_cfg.dev_override {
        Err(anyhow::anyhow!(
            "{DEV_OVERRIDE_MARK}: chunks differ from the world's"
        ))
    } else {
        DeltaStore::open(
            &args.world.join("deltas"),
            nascio_worldgen::config::GENERATOR_VERSION,
            &fingerprint,
            terrain.blocks(),
            delta_cfg.clone(),
        )
    };
    let deltas = match opened {
        Ok((d, warnings)) => {
            for w in warnings {
                warn!("deltas: {w}");
            }
            d
        }
        Err(e) => {
            error!("deltas not loaded, edits will not be saved: {e:#}");
            DeltaStore::new(
                nascio_worldgen::config::GENERATOR_VERSION,
                "",
                terrain.blocks(),
                delta_cfg,
            )
        }
    };
    let god = god::God::new(terrain.blocks().id("granite").unwrap_or(1));
    // Cloud, snow, temperature and humidity harmonics, the diurnal range and the fine elevation
    // (snow line, HUD weather) for the 3D mode.
    let climate_world = World {
        config: world.config.clone(),
        layers: world
            .layers
            .iter()
            .filter(|l| {
                [
                    "cloud_cover_",
                    "snow_fraction_",
                    "temperature_",
                    "humidity_",
                ]
                .iter()
                .any(|p| l.name.starts_with(p))
                    || l.name == "elevation"
                    || l.name == "diurnal_range"
            })
            .cloned()
            .collect(),
        files: Default::default(),
    };
    let info = fly::WorldInfo {
        astro: world.config.astro,
        clock: world.config.clock,
        seed: world.config.seed,
        lapse_rate_c_per_m: world.config.climate.lapse_rate_c_per_m,
        atmosphere: world.config.atmosphere,
        geotherm: world.config.geotherm,
    };

    let (x, z, y) = match args.pos.as_deref() {
        Some([x, z]) => (*x, *z, None),
        Some([x, y, z]) => (*x, *z, Some(*y)),
        _ => (WORLD_SIZE_F64 / 2.0, WORLD_SIZE_F64 / 2.0, None),
    };
    let ground = terrain.height(x, z).max(0.0);
    // An explicit height may be underground (HUD checks); the default stays above it.
    let y = y.unwrap_or(ground + 60.0);
    let cam = FlyCam {
        pos: WorldPos::new(x, y, z),
        yaw: args.yaw.to_radians(),
        pitch: args.pitch.to_radians(),
        speed: args.speed.clamp(fly::SPEED_MIN, fly::SPEED_MAX),
        // A capture ignores the keyboard and the mouse (reproducible pictures).
        scripted: args.screenshot.is_some(),
    };
    // Saved settings, then the command line on top (not saved). Benchmarks and captures keep
    // the defaults: reproducible runs.
    let saved = if bench_file.is_some() || args.screenshot.is_some() {
        settings::Saved::default()
    } else {
        settings::Saved::load()
    };
    let mut settings = FlySettings {
        fixed_hour: args.hour,
        fixed_day: args.day,
        fixed_clouds: args.clouds.map(|c| c.clamp(0.0, 1.0)),
        ..default()
    };
    saved.apply(&mut settings);
    if let Some(v) = args.view_distance {
        settings.view_distance_m = v.clamp(fly::VIEW_MIN, fly::VIEW_MAX);
    }
    if args.render_config.is_some() || args.curvature_radius.is_some() {
        settings.curvature_radius_m = render_cfg.radius();
    }
    settings.horizon &= !args.no_horizon;
    settings.shadows &= !args.no_shadows;
    settings.vsync &= !args.no_vsync;
    settings.fullscreen |= args.fullscreen;

    let login = menu::Login {
        name: args.name.clone().unwrap_or_else(|| saved.name.clone()),
        auth: match &args.token {
            Some(t) => nascio_client::Auth::Token(t.trim().to_owned()),
            None => nascio_client::Auth::Secret(
                args.secret.clone().unwrap_or_else(|| saved.secret.clone()),
            ),
        },
        cert: args.cert.clone(),
        known: args.known_servers.clone(),
        blocks: terrain.blocks().table().clone(),
    };
    let mut cam = cam;
    let remote = match &args.connect {
        Some(addr) => {
            anyhow::ensure!(
                !login.name.trim().is_empty(),
                "no account name: give --name (or connect once from the menu)"
            );
            let r = menu::connect(addr, &login)?;
            // Only the columns the server sends can be shown, and a column is meshed once its
            // neighbours arrived: keep one column inside the radius.
            settings.view_distance_m = settings.view_distance_m.min(r.radius_m as f32 - 64.0);
            cam.pos = WorldPos::new(r.spawn[0], r.spawn[1], r.spawn[2]);
            info!("connected to {addr}, columns within {} m", r.radius_m);
            Some(r)
        }
        None => None,
    };

    let horizon_cfg = horizon::HorizonConfig::default();
    horizon::check_layout(&horizon_cfg);

    let mut window = Window {
        title: format!(
            "{}Nascio — {}{}",
            gen_cfg.mark(),
            args.world.display(),
            args.connect
                .as_ref()
                .map_or(String::new(), |a| format!(" — server {a}"))
        ),
        present_mode: if settings.vsync {
            PresentMode::AutoVsync
        } else {
            PresentMode::AutoNoVsync
        },
        ..default()
    };
    if bench_file.is_some() || args.screenshot.is_some() {
        // Exactly 1920x1080 physical pixels whatever the desktop scaling.
        window.resolution =
            bevy::window::WindowResolution::new(1920, 1080).with_scale_factor_override(1.0);
    }
    if settings.fullscreen {
        window.mode =
            bevy::window::WindowMode::BorderlessFullscreen(bevy::window::MonitorSelection::Primary);
    }

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(window),
        ..default()
    }))
    .add_plugins(EguiPlugin::default())
    .add_plugins(curvature::CurvaturePlugin)
    .add_plugins(sky::SkyPlugin)
    .insert_resource(ClimateSource(climate_world))
    .add_plugins(perf::PerfPlugin)
    .init_state::<Mode>()
    .insert_resource(menu::Menu::new(login, saved.address.clone()))
    .insert_resource(saved)
    .init_resource::<chat::ChatBox>()
    .init_resource::<pause::Pause>()
    .insert_resource(cam)
    .insert_resource(settings)
    .insert_resource(info)
    .insert_resource(fly::SharedTerrain(terrain.clone()))
    .insert_resource(fly::Workers::new())
    .insert_resource(fly::Streamer::new())
    .insert_resource(fly::SharedDeltas(Arc::new(deltas)))
    .insert_resource(god)
    .insert_resource(horizon::Horizon::new(
        horizon_cfg,
        horizon::WaterColors::new(terrain.blocks().table()),
    ))
    .insert_resource(tiles::TileStore::new(tiles::TileSource::Local(
        terrain.clone(),
    )))
    .insert_resource(RenderOrigin(BlockPos::new(0, 0, 0)))
    .insert_resource(ClearColor(Color::srgb(0.45, 0.64, 0.9)))
    .init_resource::<fly::UiCapture>()
    // The debug panel (F3) is open at once in local flight (development).
    .insert_resource(fly::HudState {
        shown: args.fly,
        ..default()
    })
    .init_resource::<horizon::HorizonStats>()
    .add_systems(Startup, (setup, sky::setup_sky).chain())
    .add_systems(
        EguiPrimaryContextPass,
        (
            menu::menu_ui.run_if(in_state(Mode::Menu)),
            (
                god::god_ui,
                fly::hud,
                chat::chat_ui,
                pause::pause_ui,
                menu::disconnected_ui,
            )
                .chain()
                .run_if(in_state(Mode::Fly)),
        ),
    )
    .add_systems(
        Update,
        (
            fly::apply_window,
            (
                pause::pause_input,
                fly::fly_input,
                god::god_input,
                remote::remote_sync,
                fly::stream_chunks,
                horizon::update_horizon,
                fly::column_visibility,
                fly::place_camera,
                remote::draw_players,
                sky::update_sky_state,
                sky::update_light_and_clear,
                sky::update_sky,
                sky::update_star_limit,
                curvature::sync_curved,
            )
                .chain()
                .run_if(in_state(Mode::Fly)),
        )
            .chain(),
    )
    .add_systems(OnEnter(Mode::Fly), fly::reset_visibility);
    if args.fly || remote.is_some() {
        app.insert_state(Mode::Fly);
    }
    if let Some(r) = remote {
        app.insert_resource(r);
    }
    if let Some(file) = bench_file {
        app.insert_resource(bench::Bench::new(file, args.bench_out.clone()))
            .add_systems(
                Update,
                bench::drive
                    .before(fly::fly_input)
                    .run_if(in_state(Mode::Fly)),
            )
            .add_systems(Last, bench::record.after(perf::end_frame));
    }
    if let Some(path) = args.screenshot {
        app.insert_resource(AutoScreenshot {
            path,
            frame: 0,
            settled: 0,
            shot_at: None,
        })
        .add_systems(Last, auto_screenshot);
    }
    app.run();
    Ok(())
}

/// Climate layers kept for the 3D sky until [`setup`] builds [`sky::ClimateMaps`].
#[derive(Resource)]
struct ClimateSource(World);

fn setup(
    mut commands: Commands,
    source: Res<ClimateSource>,
    mut images: ResMut<Assets<Image>>,
    mut buffers: ResMut<Assets<bevy::render::storage::ShaderBuffer>>,
    mut materials: ResMut<Assets<curvature::CurvedMaterial>>,
    terrain: Res<fly::SharedTerrain>,
) {
    let maps = sky::ClimateMaps::new(&source.0, &mut images);
    let ground = sky::GroundAlbedo::new(&terrain.0, &mut images);
    let ground_handle = ground.0.clone();
    commands.insert_resource(ground);
    let climate = maps.texture.clone();
    let frame = curvature::CurvedFrame::new(&mut buffers);
    let frame_handle = frame.0.clone();
    commands.insert_resource(frame);
    commands.insert_resource(maps);
    commands.remove_resource::<ClimateSource>();
    commands.spawn((
        Camera3d::default(),
        Projection::from(PerspectiveProjection {
            // 42° vertical (pre-M5 review): less stretching at the edges (round Moon).
            fov: 42f32.to_radians(),
            near: 0.1,
            far: 60_000.0,
            ..default()
        }),
        fly::FlyCamera,
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        bevy::light::CascadeShadowConfigBuilder {
            num_cascades: 3,
            minimum_distance: 0.1,
            first_cascade_far_bound: 120.0,
            maximum_distance: 1_200.0,
            overlap_proportion: 0.2,
        }
        .build(),
        Transform::default(),
        fly::Sun,
    ));
    let opaque = materials.add(curvature::material(
        StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.95,
            reflectance: 0.15,
            // The curved materials add their own haze (same air as the sky dome).
            fog_enabled: false,
            ..default()
        },
        frame_handle.clone(),
        climate.clone(),
        ground_handle.clone(),
    ));
    let water = materials.add(curvature::material(
        StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.15,
            reflectance: 0.4,
            alpha_mode: AlphaMode::Blend,
            fog_enabled: false,
            ..default()
        },
        frame_handle.clone(),
        climate.clone(),
        ground_handle.clone(),
    ));
    commands.insert_resource(fly::TerrainMaterials { opaque, water });
    let horizon = materials.add(curvature::material(
        StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.95,
            reflectance: 0.15,
            double_sided: true,
            cull_mode: None,
            fog_enabled: false,
            ..default()
        },
        frame_handle.clone(),
        climate.clone(),
        ground_handle.clone(),
    ));
    commands.insert_resource(horizon::HorizonMaterial(horizon));
}

/// `--screenshot <file>`: capture once the scene has settled (everything in range loaded),
/// then quit.
#[derive(Resource)]
struct AutoScreenshot {
    path: PathBuf,
    frame: u32,
    settled: u32,
    shot_at: Option<u32>,
}

/// Frames the scene must stay fully loaded before the capture.
const SETTLE_FRAMES: u32 = 20;
/// Capture anyway after this many frames.
const MAX_SCREENSHOT_FRAMES: u32 = 3_000;

fn auto_screenshot(
    mut commands: Commands,
    mut shot: ResMut<AutoScreenshot>,
    state: Res<State<Mode>>,
    streamer: Res<fly::Streamer>,
    horizon: Res<horizon::HorizonStats>,
    mut exit: MessageWriter<AppExit>,
) {
    shot.frame += 1;
    let loaded = match state.get() {
        Mode::Menu => true,
        Mode::Fly => streamer.missing == 0 && horizon.pending == 0,
    };
    shot.settled = if loaded { shot.settled + 1 } else { 0 };
    match shot.shot_at {
        None if shot.settled >= SETTLE_FRAMES || shot.frame >= MAX_SCREENSHOT_FRAMES => {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(shot.path.clone()));
            shot.shot_at = Some(shot.frame);
        }
        Some(f) if shot.frame >= f + 10 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}
