//! The start dialog: an existing world (its folder) or a new one (a seed, drawn at random on
//! demand, and a folder next to the executable), and the options of the command line. The
//! choices are kept in `server-launch.toml` next to the executable for the next start; the
//! command line's options fill the form over them.
//!
//! "Démarrer" generates the new world on a thread (with its progress), loads the world and
//! starts the server ([`crate::launch`]); the window then shows the server.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use bevy::prelude::{
    AppExit, Commands, MessageWriter, NextState, Query, ResMut, Resource, Result, Window, With,
};
use bevy::window::PrimaryWindow;
use bevy_egui::{EguiContexts, egui};
use nascio_worldgen::config::{GENERATOR_VERSION, Scale, WorldConfig};
use nascio_worldgen::generate::generate_to_disk;
use nascio_worldgen::progress::{Progress, Stage};
use nascio_worldgen::world_io::World;
use serde::{Deserialize, Serialize};

use super::{Gui, Inputs, Launch, Screen, map::MapState};

/// The form, as saved in `server-launch.toml`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Form {
    /// Generate a new world (else serve an existing one).
    new_world: bool,
    /// Folder of the existing world.
    world_dir: String,
    /// Seed of the new world.
    seed: String,
    /// Resolution divisor of the new world (1, 5 or 25).
    scale: u32,
    /// Folder of the new world.
    new_dir: String,
    /// The folder of the new world follows the seed (`world-<seed>`) until edited.
    new_dir_auto: bool,
    /// The command line's options (empty: their default).
    data: String,
    config: String,
    bind: String,
    server_id: String,
    open_registration: bool,
    spawn_xz: String,
}

fn random_seed() -> String {
    rand::random::<u32>().to_string()
}

/// The executable's folder (new worlds and `server-launch.toml` go there).
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_owned))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn new_dir_for(seed: &str) -> String {
    exe_dir()
        .join(format!("world-{}", seed.trim()))
        .display()
        .to_string()
}

impl Default for Form {
    fn default() -> Self {
        let seed = random_seed();
        Self {
            new_world: false,
            world_dir: String::new(),
            new_dir: new_dir_for(&seed),
            seed,
            scale: 1,
            new_dir_auto: true,
            data: String::new(),
            config: String::new(),
            bind: String::new(),
            server_id: String::new(),
            open_registration: false,
            spawn_xz: String::new(),
        }
    }
}

fn settings_path() -> PathBuf {
    exe_dir().join("server-launch.toml")
}

/// A start under way: the generation's progress (new world), then the server's start.
struct Job {
    progress: Option<Arc<Progress>>,
    result: Mutex<Receiver<anyhow::Result<(World, Inputs)>>>,
}

#[derive(Resource)]
pub struct Setup {
    launch: Launch,
    form: Form,
    job: Option<Job>,
    error: String,
}

impl Setup {
    /// The form of the last start, then the command line over it.
    pub fn new(launch: Launch) -> Self {
        let mut form: Form = std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        let path = |p: &Option<PathBuf>| p.as_ref().map(|p| p.display().to_string());
        if let Some(w) = &launch.world {
            form.new_world = false;
            form.world_dir = w.display().to_string();
        }
        let o = &launch.opts;
        if let Some(d) = path(&o.data) {
            form.data = d;
        }
        if let Some(c) = path(&o.config) {
            form.config = c;
        }
        if let Some(b) = o.bind {
            form.bind = b.to_string();
        }
        if let Some(id) = &o.server_id {
            form.server_id = id.clone();
        }
        form.open_registration |= o.open_registration;
        if let Some([x, z]) = o.spawn_xz {
            form.spawn_xz = format!("{x},{z}");
        }
        Self {
            launch,
            form,
            job: None,
            error: String::new(),
        }
    }
}

/// What the form asks for, checked: the world folder, the new world's settings, the options.
fn read_form(f: &Form) -> anyhow::Result<(PathBuf, Option<WorldConfig>, crate::Options)> {
    let opt_path = |s: &str| (!s.trim().is_empty()).then(|| PathBuf::from(s.trim()));
    let (dir, world_cfg) = if f.new_world {
        let seed: u64 = f
            .seed
            .trim()
            .parse()
            .context("the seed is a whole number (0 to 18446744073709551615)")?;
        let dir = opt_path(&f.new_dir).context("choose the folder of the new world")?;
        anyhow::ensure!(
            !dir.join("world.toml").exists(),
            "a world already exists in {}: choose \"Monde existant\" or another folder",
            dir.display()
        );
        let scale = Scale::try_from(f.scale).map_err(anyhow::Error::msg)?;
        let mut cfg = WorldConfig::new(seed, scale);
        cfg.generator_version = GENERATOR_VERSION;
        (dir, Some(cfg))
    } else {
        let dir = opt_path(&f.world_dir).context("choose the folder of the world")?;
        anyhow::ensure!(
            dir.join("world.toml").exists(),
            "no world in {} (world.toml missing)",
            dir.display()
        );
        (dir, None)
    };
    let spawn_xz = match f.spawn_xz.trim() {
        "" => None,
        s => {
            let v: Vec<f64> = s
                .split(',')
                .map(|t| t.trim().parse::<f64>())
                .collect::<Result<_, _>>()
                .context("common spawn point: x,z in metres")?;
            anyhow::ensure!(v.len() == 2, "common spawn point: x,z in metres");
            Some([v[0], v[1]])
        }
    };
    let opts = crate::Options {
        config: opt_path(&f.config),
        data: opt_path(&f.data),
        bind: match f.bind.trim() {
            "" => None,
            b => Some(
                b.parse()
                    .with_context(|| format!("listening address {b}: ip:port"))?,
            ),
        },
        server_id: (!f.server_id.trim().is_empty()).then(|| f.server_id.trim().to_owned()),
        open_registration: f.open_registration,
        spawn_xz,
    };
    Ok((dir, world_cfg, opts))
}

/// Starts the job: generation if asked, then the server.
fn start(setup: &mut Setup) -> anyhow::Result<()> {
    let (dir, world_cfg, opts) = read_form(&setup.form)?;
    // Kept for the next start (best effort).
    if let Ok(s) = toml::to_string(&setup.form) {
        let _ = std::fs::write(settings_path(), s);
    }
    let progress = world_cfg.as_ref().map(|_| Arc::new(Progress::new()));
    let (tx, rx) = channel();
    let (rt, started, p) = (
        setup.launch.rt.clone(),
        setup.launch.started.clone(),
        progress.clone(),
    );
    std::thread::Builder::new()
        .name("start".into())
        .spawn(move || {
            let r = (|| -> anyhow::Result<(World, Inputs)> {
                if let (Some(cfg), Some(p)) = (world_cfg, p) {
                    let catalog = nascio_worldgen::deposits::Catalog::load_or_default(None)?;
                    generate_to_disk(&cfg, &catalog, &dir, &p)?;
                }
                let world = World::load(&dir)
                    .with_context(|| format!("loading the world {}", dir.display()))?;
                let running = crate::launch(&opts, &world, &dir, &rt, false)?;
                let inputs = Inputs {
                    title: format!(
                        "Nascio — serveur {} — {} — monde {} (graine {})",
                        running.shared.cfg.server_id,
                        running.addr,
                        dir.display(),
                        world.config.seed
                    ),
                    shared: running.shared.clone(),
                    stats: running.stats.clone(),
                    commands: running.commands.clone(),
                    done: running.done.clone(),
                };
                *started.lock().unwrap_or_else(|e| e.into_inner()) = Some(running);
                Ok((world, inputs))
            })();
            let _ = tx.send(r);
        })?;
    setup.job = Some(Job {
        progress,
        result: Mutex::new(rx),
    });
    setup.error.clear();
    Ok(())
}

fn clock(s: f64) -> String {
    let s = s as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[allow(clippy::too_many_arguments)]
pub fn setup_ui(
    mut contexts: EguiContexts,
    mut setup: ResMut<Setup>,
    mut commands: Commands,
    mut next: ResMut<NextState<Screen>>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    let setup = &mut *setup;
    // The job's end.
    let done = setup.job.as_ref().and_then(|j| {
        j.result
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_recv()
            .ok()
    });
    match done {
        Some(Ok((world, inputs))) => {
            if let Ok(mut w) = window.single_mut() {
                w.title = inputs.title.clone();
            }
            commands.insert_resource(MapState::new(world));
            commands.insert_resource(Gui::new(inputs));
            next.set(Screen::Running);
            setup.job = None;
            return Ok(());
        }
        Some(Err(e)) => {
            setup.error = format!("{e:#}");
            setup.job = None;
        }
        None => {}
    }

    let mut root = egui::Ui::new(
        ctx.clone(),
        "setup".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    egui::CentralPanel::default().show(&mut root, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.set_max_width(620.0);
                ui.add_space(24.0);
                ui.heading("Nascio — démarrer le serveur");
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    match &setup.job {
                        Some(job) => working(ui, job),
                        None => {
                            if form(ui, &mut setup.form)
                                && let Err(e) = start(setup)
                            {
                                setup.error = format!("{e:#}");
                            }
                            ui.add_space(6.0);
                            if ui.button("Quitter").clicked() {
                                exit.write(AppExit::Success);
                            }
                        }
                    }
                    if !setup.error.is_empty() {
                        ui.add_space(8.0);
                        ui.colored_label(egui::Color32::LIGHT_RED, &setup.error);
                    }
                });
            });
        });
    });
    if setup.job.is_some() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    Ok(())
}

/// The form; true when "Démarrer" is clicked.
fn form(ui: &mut egui::Ui, f: &mut Form) -> bool {
    ui.strong("Monde");
    ui.radio_value(&mut f.new_world, false, "Monde existant");
    ui.radio_value(&mut f.new_world, true, "Nouveau monde");
    ui.add_space(4.0);
    let wide = 440.0;
    egui::Grid::new("world")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            if f.new_world {
                ui.label("Graine");
                ui.horizontal(|ui| {
                    let seed = ui.add(egui::TextEdit::singleline(&mut f.seed).desired_width(200.0));
                    let random = ui.button("Aléatoire").clicked();
                    if random {
                        f.seed = random_seed();
                    }
                    if (seed.changed() || random) && f.new_dir_auto {
                        f.new_dir = new_dir_for(&f.seed);
                    }
                });
                ui.end_row();
                ui.label("Résolution");
                egui::ComboBox::from_id_salt("scale")
                    .selected_text(scale_label(f.scale))
                    .show_ui(ui, |ui| {
                        for s in [1, 5, 25] {
                            ui.selectable_value(&mut f.scale, s, scale_label(s));
                        }
                    });
                ui.end_row();
                ui.label("Répertoire");
                if ui
                    .add(egui::TextEdit::singleline(&mut f.new_dir).desired_width(wide))
                    .changed()
                {
                    f.new_dir_auto = false;
                }
                ui.end_row();
            } else {
                ui.label("Répertoire");
                ui.add(
                    egui::TextEdit::singleline(&mut f.world_dir)
                        .hint_text("dossier contenant world.toml")
                        .desired_width(wide),
                );
                ui.end_row();
            }
        });
    ui.add_space(8.0);
    egui::CollapsingHeader::new("Options")
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new("options")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    let row = |ui: &mut egui::Ui, label: &str, v: &mut String, hint: &str| {
                        ui.label(label);
                        ui.add(
                            egui::TextEdit::singleline(v)
                                .hint_text(hint)
                                .desired_width(wide),
                        );
                        ui.end_row();
                    };
                    row(
                        ui,
                        "Données",
                        &mut f.data,
                        "vide : le dossier du monde (comptes, éditions, certificat)",
                    );
                    row(
                        ui,
                        "Configuration",
                        &mut f.config,
                        "vide : réglages par défaut (fichier TOML)",
                    );
                    row(
                        ui,
                        "Adresse d'écoute",
                        &mut f.bind,
                        "vide : 127.0.0.1:7777 ; 0.0.0.0:7777 pour les autres machines",
                    );
                    row(
                        ui,
                        "Nom du serveur",
                        &mut f.server_id,
                        "vide : dev (seulement en écoute locale)",
                    );
                    row(
                        ui,
                        "Point d'apparition commun",
                        &mut f.spawn_xz,
                        "vide : un point par compte ; sinon x,z (tests de charge)",
                    );
                    ui.label("");
                    ui.checkbox(
                        &mut f.open_registration,
                        "Inscription ouverte (développement : nom et secret, comptes créés à la volée)",
                    );
                    ui.end_row();
                });
        });
    ui.add_space(10.0);
    ui.add(
        egui::Button::new(egui::RichText::new("Démarrer").size(18.0))
            .min_size(egui::vec2(160.0, 34.0)),
    )
    .clicked()
}

fn scale_label(s: u32) -> &'static str {
    match s {
        1 => "complète (environ 2 min)",
        5 => "1/5 (rapide)",
        _ => "1/25 (essais)",
    }
}

/// The generation's progress, then the server's start.
fn working(ui: &mut egui::Ui, job: &Job) {
    let Some(p) = &job.progress else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Démarrage du serveur…");
        });
        return;
    };
    let s = p.snapshot();
    if s.finished {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Monde généré. Démarrage du serveur…");
        });
        return;
    }
    ui.label("Génération du monde");
    ui.add(
        egui::ProgressBar::new(s.total_fraction as f32)
            .text(format!("{:.0} %", s.total_fraction * 100.0)),
    );
    if let Some(stage) = s.stage {
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
    ui.monospace(format!("écoulé {}", clock(s.elapsed_s)));
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
}
