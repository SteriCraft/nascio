//! The server's window (feature `gui`). It opens on the start dialog ([`setup`]): an existing
//! world or a new one, and the options. Once the server runs: the world map in the middle, its
//! aspects on the left, the console at the bottom (its height can be dragged), measurements
//! and the players online on the right.
//!
//! The window reads the server's state (players, bans, measurements) and acts only through
//! console commands, sent to the console task like lines typed on the terminal: every action
//! leaves its line in the console. A right click on a player bans them or writes to them.

mod charts;
mod map;
mod setup;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy::winit::{UpdateMode, WinitSettings};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use nascio_server::net::{Shared, unix_ms};
use nascio_server::sim::SimStats;
use tokio::sync::mpsc::UnboundedSender;

/// What the window gets from `main`.
pub struct Launch {
    /// World folder of the command line (proposed by the start dialog).
    pub world: Option<PathBuf>,
    /// Options of the command line (they fill the start dialog).
    pub opts: crate::Options,
    pub rt: tokio::runtime::Handle,
    /// The server, once started: `main` stops it when the window closes.
    pub started: Arc<Mutex<Option<crate::Running>>>,
}

/// What the window needs from the running server.
pub struct Inputs {
    pub title: String,
    pub shared: Arc<Shared>,
    pub stats: Arc<Mutex<SimStats>>,
    /// Console commands (run by the console task).
    pub commands: UnboundedSender<String>,
    /// Set once the server stopped (`stop`, Ctrl+C): the window closes.
    pub done: Arc<AtomicBool>,
}

/// The start dialog, then the running server.
#[derive(States, Default, Clone, Copy, Eq, PartialEq, Hash, Debug)]
enum Screen {
    #[default]
    Setup,
    Running,
}

/// Durations of a ban offered by the dialog: label, console argument.
const BAN_DURATIONS: [(&str, &str); 5] = [
    ("définitif", ""),
    ("1 heure", "1h"),
    ("1 jour", "1d"),
    ("7 jours", "7d"),
    ("30 jours", "30d"),
];

/// Dialog banning a player.
struct BanDialog {
    name: String,
    duration: usize,
    reason: String,
}

/// Dialog writing to a player.
struct TellDialog {
    name: String,
    text: String,
}

#[derive(Resource)]
struct Gui {
    inputs: Inputs,
    /// Console lines shown, and the number of the next one to read.
    lines: VecDeque<String>,
    next_line: u64,
    input: String,
    /// Commands typed, for the up and down arrows.
    history: Vec<String>,
    history_at: usize,
    charts: charts::History,
    /// Player selected in the list (highlighted on the map).
    selected: Option<String>,
    bans_open: bool,
    ban_selected: Option<String>,
    ban_dialog: Option<BanDialog>,
    tell_dialog: Option<TellDialog>,
}

impl Gui {
    fn new(inputs: Inputs) -> Self {
        Self {
            inputs,
            lines: VecDeque::new(),
            next_line: 0,
            input: String::new(),
            history: Vec::new(),
            history_at: 0,
            charts: charts::History::default(),
            selected: None,
            bans_open: false,
            ban_selected: None,
            ban_dialog: None,
            tell_dialog: None,
        }
    }

    fn command(&self, line: String) {
        let _ = self.inputs.commands.send(line);
    }
}

/// Opens the window and runs it until it is closed or the server stops.
pub fn run(launch: Launch) {
    let window = Window {
        title: "Nascio — serveur".into(),
        resolution: bevy::window::WindowResolution::new(1600, 960),
        ..default()
    };
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(window),
                    ..default()
                })
                .set(LogPlugin {
                    level: Level::WARN,
                    ..default()
                }),
        )
        .add_plugins(EguiPlugin::default())
        // A window of status: a few frames a second are enough, fewer when out of focus.
        .insert_resource(WinitSettings {
            focused_mode: UpdateMode::reactive(Duration::from_millis(100)),
            unfocused_mode: UpdateMode::reactive_low_power(Duration::from_millis(500)),
        })
        .insert_resource(ClearColor(Color::srgb(0.1, 0.1, 0.11)))
        .insert_resource(setup::Setup::new(launch))
        .init_state::<Screen>()
        .add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
        })
        .add_systems(
            Update,
            (sample, close_when_done).run_if(in_state(Screen::Running)),
        )
        .add_systems(
            EguiPrimaryContextPass,
            (
                setup::setup_ui.run_if(in_state(Screen::Setup)),
                ui.run_if(in_state(Screen::Running)),
            ),
        )
        .run();
}

/// Closes the window once the server stopped.
fn close_when_done(gui: Res<Gui>, mut exit: MessageWriter<AppExit>) {
    if gui.inputs.done.load(Ordering::Relaxed) {
        exit.write(AppExit::Success);
    }
}

/// Measurements once a second, new console lines every frame.
fn sample(mut gui: ResMut<Gui>, time: Res<Time<Real>>) {
    let gui = &mut *gui;
    let (lines, next) = nascio_server::console::since(gui.next_line);
    gui.next_line = next;
    gui.lines.extend(lines);
    while gui.lines.len() > nascio_server::console::KEPT {
        gui.lines.pop_front();
    }
    gui.charts.update(
        time.elapsed_secs_f64(),
        &gui.inputs.shared,
        &gui.inputs.stats,
    );
}

fn ui(
    mut contexts: EguiContexts,
    mut gui: ResMut<Gui>,
    mut map: ResMut<map::MapState>,
    time: Res<Time<Real>>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    let gui = &mut *gui;
    map.advance_time(time.delta_secs_f64());
    let sh = gui.inputs.shared.clone();
    let players: Vec<map::Dot> = sh
        .list()
        .into_iter()
        .map(|(name, _, pos)| map::Dot { name, pos })
        .collect();
    // A selected player who left is no longer selected.
    if let Some(n) = &gui.selected
        && !players.iter().any(|p| &p.name == n)
    {
        gui.selected = None;
    }
    let sites: Vec<(f64, f64)> = sh
        .spawns
        .sites
        .iter()
        .chain(sh.spawns.fixed.iter())
        .map(|p| (f64::from(p.x) + 0.5, f64::from(p.z) + 0.5))
        .collect();

    let mut root = egui::Ui::new(
        ctx.clone(),
        "viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    egui::Panel::bottom("console")
        .resizable(true)
        .default_size(220.0)
        .min_size(90.0)
        .show(&mut root, |ui| console(ui, gui));
    egui::Panel::left("aspects")
        .resizable(false)
        .min_size(240.0)
        .show(&mut root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| map::side_panel(ui, &mut map));
        });
    egui::Panel::right("status")
        .resizable(true)
        .default_size(320.0)
        .min_size(260.0)
        .show(&mut root, |ui| status(ui, gui, &players));
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(&mut root, |ui| {
            map::map(ui, &mut map, &players, gui.selected.as_deref(), &sites);
        });
    bans_window(&ctx, gui);
    ban_dialog(&ctx, gui);
    tell_dialog(&ctx, gui);
    if map.animated() {
        ctx.request_repaint();
    }
    Ok(())
}

/// The console: its lines, and a line to type commands (Enter runs it, arrows recall).
fn console(ui: &mut egui::Ui, gui: &mut Gui) {
    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
        ui.add_space(4.0);
        let edit = ui.add(
            egui::TextEdit::singleline(&mut gui.input)
                .font(egui::TextStyle::Monospace)
                .hint_text("commande (help pour la liste, message <texte> pour écrire à tous)")
                .desired_width(f32::INFINITY),
        );
        if edit.has_focus() {
            let (up, down) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::ArrowDown),
                )
            });
            if up && gui.history_at > 0 {
                gui.history_at -= 1;
                gui.input = gui.history[gui.history_at].clone();
            }
            if down && gui.history_at < gui.history.len() {
                gui.history_at += 1;
                gui.input = gui.history.get(gui.history_at).cloned().unwrap_or_default();
            }
        }
        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let line = std::mem::take(&mut gui.input);
            if !line.trim().is_empty() {
                gui.history.push(line.clone());
                gui.history_at = gui.history.len();
                gui.command(line);
            }
            edit.request_focus();
        }
        ui.separator();
        let row = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show_rows(ui, row, gui.lines.len(), |ui, range| {
                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    for l in gui.lines.range(range) {
                        ui.monospace(l);
                    }
                });
            });
    });
}

/// The right section: measurements on top, the players online below.
fn status(ui: &mut egui::Ui, gui: &mut Gui, players: &[map::Dot]) {
    ui.heading("Performances");
    charts::show(ui, &gui.charts);
    ui.separator();
    ui.horizontal(|ui| {
        ui.heading(format!("Joueurs ({})", players.len()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Bannis")
                .on_hover_text("Liste des joueurs bannis")
                .clicked()
            {
                gui.bans_open = !gui.bans_open;
            }
        });
    });
    ui.small("Clic : voir sur la carte · clic droit : bannir, écrire");
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            for p in players {
                let on = gui.selected.as_deref() == Some(p.name.as_str());
                let r = ui
                    .add(
                        egui::Button::selectable(on, p.name.as_str())
                            .min_size(egui::vec2(ui.available_width(), 0.0)),
                    )
                    .on_hover_text(format!(
                        "x {:.0} · y {:.0} · z {:.0}",
                        p.pos[0], p.pos[1], p.pos[2]
                    ));
                if r.clicked() {
                    gui.selected = if on { None } else { Some(p.name.clone()) };
                }
                r.context_menu(|ui| {
                    if ui.button("Bannir…").clicked() {
                        gui.ban_dialog = Some(BanDialog {
                            name: p.name.clone(),
                            duration: 0,
                            reason: String::new(),
                        });
                        ui.close();
                    }
                    if ui.button("Envoyer un message…").clicked() {
                        gui.tell_dialog = Some(TellDialog {
                            name: p.name.clone(),
                            text: String::new(),
                        });
                        ui.close();
                    }
                });
            }
        });
}

/// The banned accounts; the selected one can be unbanned.
fn bans_window(ctx: &egui::Context, gui: &mut Gui) {
    if !gui.bans_open {
        return;
    }
    let now = unix_ms();
    let bans = gui.inputs.shared.accounts.bans(now);
    if let Some(n) = &gui.ban_selected
        && !bans.iter().any(|(b, _)| b == n)
    {
        gui.ban_selected = None;
    }
    let mut open = true;
    egui::Window::new("Joueurs bannis")
        .open(&mut open)
        .default_size([360.0, 300.0])
        .show(ctx, |ui| {
            ui.with_layout(egui::Layout::bottom_up(egui::Align::RIGHT), |ui| {
                let can = gui.ban_selected.is_some();
                if ui.add_enabled(can, egui::Button::new("Débannir")).clicked()
                    && let Some(name) = gui.ban_selected.take()
                {
                    gui.command(format!("unban {name}"));
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                            if bans.is_empty() {
                                ui.label("Aucun joueur banni.");
                            }
                            for (name, ban) in &bans {
                                let on = gui.ban_selected.as_deref() == Some(name.as_str());
                                let r = ui
                                    .add(
                                        egui::Button::selectable(on, name.as_str())
                                            .min_size(egui::vec2(ui.available_width(), 0.0)),
                                    )
                                    .on_hover_text(ban.message(now));
                                if r.clicked() {
                                    gui.ban_selected = if on { None } else { Some(name.clone()) };
                                }
                            }
                        });
                    });
            });
        });
    gui.bans_open = open;
}

fn ban_dialog(ctx: &egui::Context, gui: &mut Gui) {
    let Some(d) = &mut gui.ban_dialog else {
        return;
    };
    let mut close = false;
    let mut command = None;
    egui::Window::new(format!("Bannir {}", d.name))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::ComboBox::from_label("Durée")
                .selected_text(BAN_DURATIONS[d.duration].0)
                .show_ui(ui, |ui| {
                    for (k, (label, _)) in BAN_DURATIONS.iter().enumerate() {
                        ui.selectable_value(&mut d.duration, k, *label);
                    }
                });
            ui.label("Raison (facultative)");
            ui.text_edit_singleline(&mut d.reason);
            ui.horizontal(|ui| {
                if ui.button("Bannir").clicked() {
                    let dur = BAN_DURATIONS[d.duration].1;
                    let mut c = format!("ban {}", d.name);
                    for part in [dur, d.reason.trim()] {
                        if !part.is_empty() {
                            c.push(' ');
                            c.push_str(part);
                        }
                    }
                    command = Some(c);
                    close = true;
                }
                if ui.button("Annuler").clicked() {
                    close = true;
                }
            });
        });
    if let Some(c) = command {
        gui.command(c);
    }
    if close {
        gui.ban_dialog = None;
    }
}

fn tell_dialog(ctx: &egui::Context, gui: &mut Gui) {
    let Some(d) = &mut gui.tell_dialog else {
        return;
    };
    let mut close = false;
    let mut command = None;
    egui::Window::new(format!("Message à {}", d.name))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Seul ce joueur le lira, sous la forme [Admin]: <message>.");
            let edit = ui.add(egui::TextEdit::singleline(&mut d.text).desired_width(320.0));
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                if (ui.button("Envoyer").clicked() || enter) && !d.text.trim().is_empty() {
                    command = Some(format!("tell {} {}", d.name, d.text.trim()));
                    close = true;
                }
                if ui.button("Annuler").clicked() {
                    close = true;
                }
            });
        });
    if let Some(c) = command {
        gui.command(c);
    }
    if close {
        gui.tell_dialog = None;
    }
}
