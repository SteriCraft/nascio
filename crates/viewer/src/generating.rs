//! Generation window (`--generate SEED`): generates a world on a worker thread while a small
//! window shows its progress (overall percentage, running stage and its own progress, elapsed
//! time), then closes. The window only reads the progress counters.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bevy::prelude::*;
use bevy::window::WindowResolution;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use nascio_worldgen::config::{GENERATOR_VERSION, Scale, WorldConfig};
use nascio_worldgen::generate::generate_to_disk;
use nascio_worldgen::progress::{Progress, Stage};

#[derive(Resource)]
struct Job {
    seed: u64,
    out: PathBuf,
    progress: Arc<Progress>,
    /// Seconds the window stays after success (then the app exits).
    linger: f64,
    closing: Option<f64>,
    /// Capture of the window once the progress passes 30 % and the window has drawn (checks).
    screenshot: Option<PathBuf>,
}

/// Generates world `seed` at `scale` (optional `config` TOML) into `out`, with the progress
/// window. Returns once the window is closed; an error if the generation failed.
pub fn run(
    seed: u64,
    scale: u32,
    config: Option<PathBuf>,
    catalog: Option<PathBuf>,
    out: PathBuf,
    screenshot: Option<PathBuf>,
) -> anyhow::Result<()> {
    let scale = Scale::try_from(scale).map_err(anyhow::Error::msg)?;
    let mut cfg = match config {
        Some(p) => {
            let mut c = WorldConfig::from_toml(&std::fs::read_to_string(&p)?)?;
            c.seed = seed;
            c.scale = scale;
            c
        }
        None => WorldConfig::new(seed, scale),
    };
    cfg.generator_version = GENERATOR_VERSION;
    let catalog = nascio_worldgen::deposits::Catalog::load_or_default(catalog.as_deref())?;
    let progress = Arc::new(Progress::new());
    {
        let (progress, out) = (progress.clone(), out.clone());
        std::thread::spawn(move || {
            let _ = generate_to_disk(&cfg, &catalog, &out, &progress);
        });
    }
    let window = Window {
        title: format!("Nascio — generating world {seed}"),
        resolution: WindowResolution::new(560, 420),
        resizable: false,
        ..default()
    };
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(window),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .insert_resource(ClearColor(Color::srgb(0.08, 0.09, 0.11)))
        .insert_resource(Job {
            seed,
            out,
            progress: progress.clone(),
            linger: 1.0,
            closing: None,
            screenshot,
        })
        .add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
        })
        .add_systems(EguiPrimaryContextPass, ui)
        .run();
    let s = progress.snapshot();
    match s.error {
        Some(e) => anyhow::bail!("generation failed: {e}"),
        None if !s.finished => anyhow::bail!("generation window closed before the end"),
        None => {
            println!("{}", progress.line());
            Ok(())
        }
    }
}

fn clock(s: f64) -> String {
    let s = s as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn ui(
    mut commands: Commands,
    mut contexts: EguiContexts,
    mut job: ResMut<Job>,
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let s = job.progress.snapshot();
    let mut root = egui::Ui::new(
        ctx.clone(),
        "generating".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    egui::CentralPanel::default().show(&mut root, |ui| {
        ui.heading(format!("Generating world {}", job.seed));
        ui.label(job.out.display().to_string());
        ui.add_space(8.0);
        ui.add(
            egui::ProgressBar::new(s.total_fraction as f32)
                .text(format!("{:.0} %", s.total_fraction * 100.0)),
        );
        ui.add_space(4.0);
        match s.stage {
            Some(stage) => {
                let detail = if s.detail.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", s.detail)
                };
                ui.label(format!("{}{detail}", stage.name()));
                ui.add(
                    egui::ProgressBar::new(s.stage_fraction as f32)
                        .text(format!("{:.0} %", s.stage_fraction * 100.0)),
                );
            }
            None if s.error.is_some() => {
                ui.colored_label(egui::Color32::LIGHT_RED, "failed");
            }
            None if s.finished => {
                ui.label("done");
            }
            None => {
                ui.label("starting");
            }
        }
        ui.add_space(4.0);
        ui.monospace(format!("elapsed {}", clock(s.elapsed_s)));
        ui.separator();
        egui::Grid::new("stages").num_columns(2).show(ui, |ui| {
            for st in Stage::ALL {
                let done = s.done.iter().find(|d| d.0 == st);
                let (mark, t) = match done {
                    Some(d) => ("✔", format!("{:.1} s", d.1)),
                    None if s.stage == Some(st) => ("▶", String::new()),
                    None => ("·", String::new()),
                };
                ui.monospace(format!("{mark} {}", st.name()));
                ui.monospace(t);
                ui.end_row();
            }
        });
        if let Some(e) = &s.error {
            ui.separator();
            ui.colored_label(egui::Color32::LIGHT_RED, e);
            ui.label("Close the window to quit.");
        }
    });
    if s.total_fraction > 0.3
        && time.elapsed_secs() > 2.0
        && let Some(path) = job.screenshot.take()
    {
        use bevy::render::view::window::screenshot::{Screenshot, save_to_disk};
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if s.finished && s.error.is_none() {
        let now = time.elapsed_secs_f64();
        let linger = job.linger;
        let at = *job.closing.get_or_insert(now + linger);
        if now >= at {
            exit.write(AppExit::Success);
        }
    }
    ctx.request_repaint_after(Duration::from_millis(100));
    Ok(())
}
