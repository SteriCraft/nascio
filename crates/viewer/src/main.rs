//! Nascio development viewer: map mode (M1) and free-flight 3D mode (M2).
//!
//! ```text
//! cargo run -p nascio-viewer --release -- out/m2/world-42
//! cargo run -p nascio-viewer --release -- out/m2/world-42 --fly --pos 21000,1500
//! ```

mod bench;
mod curvature;
mod fly;
mod generating;
mod horizon;
mod map;
mod perf;
mod sky;
mod stars;

use std::path::PathBuf;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::view::window::screenshot::{Screenshot, save_to_disk};
use bevy::window::PresentMode;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};
use clap::Parser;
use nascio_core::consts::WORLD_SIZE_F64;
use nascio_core::coords::{BlockPos, WorldPos};
use nascio_voxel::{Terrain, VoxelConfig};
use nascio_worldgen::world_io::World;

use fly::{FlyCam, FlySettings, RenderOrigin};

#[derive(Parser, Debug)]
#[command(about = "Nascio world viewer")]
struct Args {
    /// World directory (from `nascio-worldgen generate`).
    #[arg(default_value = "out/world-42")]
    world: PathBuf,
    /// Start in the 3D mode.
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
    /// Chunk view distance, m (1 024 m by default, the slider's maximum).
    #[arg(long, default_value_t = 1_024.0)]
    view_distance: f32,
    /// Fixed local solar hour for lighting (default: real time).
    #[arg(long)]
    hour: Option<f64>,
    /// Day of the year (0..365) for the map date and the 3D sky (default: real time).
    #[arg(long)]
    day: Option<f64>,
    /// Map mode: layer or climate map shown at start (e.g. `temperature`, `biome`).
    #[arg(long)]
    map_layer: Option<String>,
    /// Disable the sun shadows.
    #[arg(long)]
    no_shadows: bool,
    /// Disable the horizon.
    #[arg(long)]
    no_horizon: bool,
    /// Capture the window once the scene is loaded, then quit.
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// TOML file with a `[voxel]` section overriding chunk generation parameters.
    #[arg(long)]
    voxel_config: Option<PathBuf>,
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
    /// Map mode: open the vertical section `x0,z0,x1,z1` at start.
    #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
    section: Option<Vec<f64>>,
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

#[derive(States, Default, Clone, Copy, Eq, PartialEq, Hash, Debug)]
enum Mode {
    #[default]
    Map,
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
        args.view_distance = b.view_distance_m.unwrap_or(fly::VIEW_MAX);
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
    let voxel_cfg = match &args.voxel_config {
        Some(p) => VoxelConfig::from_toml(&std::fs::read_to_string(p)?)?,
        None => VoxelConfig::default(),
    };
    let mut render_cfg = match &args.render_config {
        Some(p) => curvature::RenderConfig::from_toml(&std::fs::read_to_string(p)?)?,
        None => curvature::RenderConfig::default(),
    };
    if let Some(r) = args.curvature_radius {
        render_cfg.curvature_radius_m = r;
    }
    let terrain = Arc::new(Terrain::from_world(&world, voxel_cfg)?);
    // Cloud, snow, temperature and humidity harmonics, the diurnal range and the fine elevation
    // (snow line, HUD weather) for the 3D mode (the map keeps the world).
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
    let settings = FlySettings {
        view_distance_m: args.view_distance.clamp(fly::VIEW_MIN, fly::VIEW_MAX),
        fixed_hour: args.hour,
        fixed_day: args.day,
        horizon: !args.no_horizon,
        curvature_radius_m: render_cfg.radius(),
        shadows: !args.no_shadows,
        vsync: !args.no_vsync,
        ..default()
    };

    let horizon_cfg = horizon::HorizonConfig::default();
    horizon::check_layout(&horizon_cfg);

    let mut map = map::MapState::new(world, args.world.clone());
    map.center = (cam.pos.x, cam.pos.z);
    if let Some(name) = &args.map_layer {
        map.select(name);
    }
    if let Some(day) = args.day {
        map.set_date(day, args.hour.unwrap_or(12.0));
    }
    if let Some(v) = &args.section {
        anyhow::ensure!(v.len() == 4, "--section takes x0,z0,x1,z1");
        map.open_section((v[0], v[1]), (v[2], v[3]));
    }

    let mut window = Window {
        title: format!("Nascio viewer — {}", args.world.display()),
        present_mode: if args.no_vsync {
            PresentMode::AutoNoVsync
        } else {
            PresentMode::AutoVsync
        },
        ..default()
    };
    if bench_file.is_some() {
        // Exactly 1920x1080 physical pixels whatever the desktop scaling.
        window.resolution =
            bevy::window::WindowResolution::new(1920, 1080).with_scale_factor_override(1.0);
    }
    if args.fullscreen {
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
    .insert_resource(map)
    .insert_resource(cam)
    .insert_resource(settings)
    .insert_resource(info)
    .insert_resource(fly::SharedTerrain(terrain))
    .insert_resource(fly::Workers::new())
    .insert_resource(fly::Streamer::new())
    .insert_resource(horizon::Horizon::new(horizon_cfg))
    .insert_resource(RenderOrigin(BlockPos::new(0, 0, 0)))
    .insert_resource(ClearColor(Color::srgb(0.45, 0.64, 0.9)))
    .init_resource::<fly::UiCapture>()
    .init_resource::<fly::HudState>()
    .init_resource::<horizon::HorizonStats>()
    .add_systems(Startup, (setup, sky::setup_sky))
    .add_systems(
        EguiPrimaryContextPass,
        (
            map::ui.run_if(in_state(Mode::Map)),
            fly::hud.run_if(in_state(Mode::Fly)),
        ),
    )
    .add_systems(
        Update,
        (
            switch_mode,
            fly::apply_vsync,
            (
                fly::fly_input,
                fly::stream_chunks,
                horizon::update_horizon,
                fly::column_visibility,
                fly::place_camera,
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
    .add_systems(OnEnter(Mode::Fly), fly::reset_visibility)
    .add_systems(OnEnter(Mode::Map), hide_scene);
    if args.fly {
        app.insert_state(Mode::Fly);
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
) {
    let maps = sky::ClimateMaps::new(&source.0, &mut images);
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
    ));
    commands.insert_resource(horizon::HorizonMaterial(horizon));
}

/// Map and 3D share one camera: the 3D scene is hidden behind the map. Entering the 3D mode,
/// chunk and horizon visibility are restored by their own systems.
fn hide_scene(mut q: Query<&mut Visibility, With<fly::Anchor>>) {
    for mut v in &mut q {
        *v = Visibility::Hidden;
    }
}

fn switch_mode(
    state: Res<State<Mode>>,
    mut next: ResMut<NextState<Mode>>,
    mut map: ResMut<map::MapState>,
    mut hud: ResMut<fly::HudState>,
    mut cam: ResMut<FlyCam>,
    terrain: Res<fly::SharedTerrain>,
) {
    match state.get() {
        Mode::Map if map.fly_request.is_some() => {
            let (x, z) = map.fly_request.take().unwrap_or(map.center);
            cam.pos = WorldPos::new(x, terrain.0.height(x, z).max(0.0) + 60.0, z);
            next.set(Mode::Fly);
        }
        Mode::Fly if hud.map_request => {
            hud.map_request = false;
            map.center = (cam.pos.x, cam.pos.z);
            next.set(Mode::Map);
        }
        _ => {}
    }
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
        Mode::Map => true,
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
