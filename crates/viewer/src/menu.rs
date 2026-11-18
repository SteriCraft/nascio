//! The opening menu of the client: "Se connecter", "Paramètres", "Quitter".
//!
//! "Se connecter" opens a dialog asking for the server's address (127.0.0.1 by default), with
//! a button scanning the local network, and the account (name and secret). The connection runs
//! on a thread of its own; once the server accepted it, the account and the address are saved
//! ([`crate::settings`]) and the 3D view opens at the player's spawn point. "Paramètres" opens
//! the display settings.
//!
//! The server's certificate comes with its answer to the scan probe
//! (`nascio_protocol::discovery`) and is pinned at the first connection to an address
//! (`known_servers.toml`): a later different certificate is refused. `--cert` gives it from a
//! file instead.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use anyhow::Context;
use bevy::prelude::{AppExit, Commands, MessageWriter, NextState, ResMut, Resource, Result, info};
use bevy_egui::{EguiContexts, egui};
use nascio_chunk::Blocks;
use nascio_client::Auth;
use nascio_client::discovery::{self, Found};
use serde::{Deserialize, Serialize};

use crate::Mode;
use crate::fly::{FlyCam, FlySettings};
use crate::remote::Remote;
use crate::settings::Saved;

/// How the client logs in: the saved account, or the command line's.
#[derive(Clone)]
pub struct Login {
    pub name: String,
    pub auth: Auth,
    /// Certificate file given on the command line (else the scan's answer, pinned).
    pub cert: Option<PathBuf>,
    /// Certificates pinned by address.
    pub known: PathBuf,
    /// Block table of the local world (the server's must be the same).
    pub blocks: Blocks,
}

#[derive(Resource)]
pub struct Menu {
    pub login: Login,
    dialog: bool,
    settings: bool,
    address: String,
    scanning: Option<Mutex<Receiver<anyhow::Result<Vec<Found>>>>>,
    found: Vec<Found>,
    scan_status: String,
    connecting: Option<Mutex<Receiver<anyhow::Result<Remote>>>>,
    status: String,
}

impl Menu {
    pub fn new(login: Login, address: String) -> Self {
        Self {
            login,
            dialog: false,
            settings: false,
            address,
            scanning: None,
            found: Vec::new(),
            scan_status: String::new(),
            connecting: None,
            status: String::new(),
        }
    }
}

/// Pinned certificates: address → certificate (DER, hexadecimal).
#[derive(Default, Serialize, Deserialize)]
struct Known {
    #[serde(default)]
    servers: BTreeMap<String, String>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Checks `cert` against the one pinned for `addr`, pins it if there is none.
fn pin(path: &PathBuf, addr: SocketAddr, cert: &[u8]) -> anyhow::Result<()> {
    let mut known: Known = match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).with_context(|| format!("reading {}", path.display()))?,
        Err(_) => Known::default(),
    };
    let key = addr.to_string();
    match known.servers.get(&key) {
        Some(h) if *h == hex(cert) => Ok(()),
        Some(_) => anyhow::bail!(
            "the certificate of {key} changed since the first connection: remove its line from {} if the server was really reinstalled",
            path.display()
        ),
        None => {
            known.servers.insert(key, hex(cert));
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(path, toml::to_string(&known)?)?;
            Ok(())
        }
    }
}

/// Connects to `address` (`ip`, `host`, `ip:port`): the game port and the certificate come
/// from the server's answer to a probe, unless `--cert` gave the certificate.
pub fn connect(address: &str, login: &Login) -> anyhow::Result<Remote> {
    let address = address.trim();
    // An address without a port: the server's answer tells it.
    let with_port = address.parse::<SocketAddr>().ok().or_else(|| {
        address
            .contains(':')
            .then(|| address.to_socket_addrs().ok()?.next())
            .flatten()
    });
    let ip: IpAddr = match with_port {
        Some(a) => a.ip(),
        None => address
            .parse()
            .ok()
            .or_else(|| (address, 0).to_socket_addrs().ok()?.map(|a| a.ip()).next())
            .with_context(|| format!("unknown address {address}"))?,
    };
    let (addr, cert) = match &login.cert {
        Some(file) => {
            let cert =
                std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
            let addr = with_port.unwrap_or(SocketAddr::new(ip, 7777));
            (addr, cert)
        }
        None => {
            let found = discovery::query(ip, Duration::from_secs(2))?;
            let addr = with_port.unwrap_or(found.addr);
            pin(&login.known, addr, &found.info.cert_der)?;
            (addr, found.info.cert_der)
        }
    };
    Remote::start(addr, cert, &login.name, login.auth.clone(), &login.blocks)
}

/// The result of a thread, if it came.
fn try_take<T>(rx: &Mutex<Receiver<T>>) -> Option<T> {
    rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv().ok()
}

/// The menu, the connection dialog, and the switch to the 3D view once connected.
#[allow(clippy::too_many_arguments)]
pub fn menu_ui(
    mut contexts: EguiContexts,
    mut menu: ResMut<Menu>,
    mut saved: ResMut<Saved>,
    mut commands: Commands,
    mut next: ResMut<NextState<Mode>>,
    mut cam: ResMut<FlyCam>,
    mut settings: ResMut<FlySettings>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let menu = &mut *menu;
    if let Some(r) = menu.scanning.as_ref().and_then(try_take) {
        menu.scanning = None;
        match r {
            Ok(found) => {
                menu.scan_status = match found.len() {
                    0 => "Aucun serveur trouvé.".into(),
                    1 => "1 serveur trouvé.".into(),
                    n => format!("{n} serveurs trouvés."),
                };
                menu.found = found;
            }
            Err(e) => menu.scan_status = format!("Échec du scan : {e:#}"),
        }
    }
    if let Some(r) = menu.connecting.as_ref().and_then(try_take) {
        menu.connecting = None;
        match r {
            Ok(remote) => {
                let p = remote.spawn;
                cam.pos = nascio_core::coords::WorldPos::new(p[0], p[1], p[2]);
                // Only the columns the server sends can be shown, and a column is meshed once
                // its neighbours arrived: keep one column inside the radius.
                settings.view_distance_m =
                    settings.view_distance_m.min(remote.radius_m as f32 - 64.0);
                info!("connected, columns within {} m", remote.radius_m);
                saved.name = menu.login.name.trim().to_owned();
                if let Auth::Secret(secret) = &menu.login.auth {
                    saved.secret = secret.clone();
                }
                saved.address = menu.address.trim().to_owned();
                saved.save();
                commands.insert_resource(remote);
                menu.dialog = false;
                next.set(Mode::Fly);
            }
            Err(e) => menu.status = format!("Échec : {e:#}"),
        }
    }

    let mut root = egui::Ui::new(
        ctx.clone(),
        "menu".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(18, 24, 34)))
        .show(&mut root, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.25);
                ui.label(egui::RichText::new("Nascio").size(56.0).strong());
                ui.add_space(36.0);
                let size = egui::vec2(260.0, 44.0);
                let big = |t: &str| egui::Button::new(egui::RichText::new(t).size(20.0));
                if ui.add(big("Se connecter").min_size(size)).clicked() {
                    menu.dialog = true;
                    menu.settings = false;
                }
                ui.add_space(10.0);
                if ui.add(big("Paramètres").min_size(size)).clicked() {
                    menu.settings = true;
                    menu.dialog = false;
                }
                ui.add_space(10.0);
                if ui.add(big("Quitter").min_size(size)).clicked() {
                    exit.write(AppExit::Success);
                }
            });
        });

    if menu.dialog {
        let mut open = true;
        egui::Window::new("Se connecter")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 80.0])
            .show(ctx, |ui| connect_dialog(ui, menu));
        if !open && menu.connecting.is_none() {
            menu.dialog = false;
        }
    }
    crate::settings::settings_window(ctx, &mut menu.settings, &mut saved, &mut settings);
    if menu.scanning.is_some() || menu.connecting.is_some() {
        ctx.request_repaint();
    }
    Ok(())
}

fn connect_dialog(ui: &mut egui::Ui, menu: &mut Menu) {
    let busy = menu.connecting.is_some();
    ui.label("Adresse du serveur");
    let edit = ui.add_enabled(
        !busy,
        egui::TextEdit::singleline(&mut menu.address).desired_width(280.0),
    );
    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.add_space(6.0);
    let scanning = menu.scanning.is_some();
    if ui
        .add_enabled(
            !busy && !scanning,
            egui::Button::new("Scanner le réseau local"),
        )
        .clicked()
    {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(discovery::scan(Duration::from_millis(1_500)));
        });
        menu.scanning = Some(Mutex::new(rx));
        menu.scan_status = "Recherche…".into();
        menu.found.clear();
    }
    if !menu.scan_status.is_empty() {
        ui.label(&menu.scan_status);
    }
    for f in &menu.found {
        let text = format!(
            "{} — {} — {}/{} joueurs",
            f.info.server_id, f.addr, f.info.players, f.info.max_players
        );
        let on = menu.address == f.addr.to_string();
        if ui
            .add_enabled(!busy, egui::Button::selectable(on, text))
            .clicked()
        {
            menu.address = f.addr.to_string();
        }
    }
    ui.add_space(6.0);
    let mut enter = enter;
    egui::Grid::new("account").num_columns(2).show(ui, |ui| {
        ui.label("Nom du compte");
        let r = ui.add_enabled(
            !busy,
            egui::TextEdit::singleline(&mut menu.login.name).desired_width(200.0),
        );
        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.end_row();
        match &mut menu.login.auth {
            Auth::Secret(secret) => {
                ui.label("Mot de passe");
                let r = ui.add_enabled(
                    !busy,
                    egui::TextEdit::singleline(secret)
                        .password(true)
                        .desired_width(200.0),
                );
                enter |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            }
            Auth::Token(_) => {
                ui.label("Jeton");
                ui.label("donné en ligne de commande");
            }
        }
        ui.end_row();
    });
    ui.small("Le compte est créé à la première connexion si le serveur le permet.");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let go = ui
            .add_enabled(!busy, egui::Button::new("Connexion"))
            .clicked();
        if (go || (enter && !busy)) && !menu.address.trim().is_empty() {
            if let Err(e) = check_account(&menu.login) {
                menu.status = e.into();
                return;
            }
            let (tx, rx) = channel();
            let (address, mut login) = (menu.address.clone(), menu.login.clone());
            login.name = login.name.trim().to_owned();
            std::thread::spawn(move || {
                let _ = tx.send(connect(&address, &login));
            });
            menu.connecting = Some(Mutex::new(rx));
            menu.status = "Connexion…".into();
        }
        if busy {
            ui.spinner();
        }
    });
    if !menu.status.is_empty() {
        ui.label(&menu.status);
    }
}

/// What the server would refuse anyway, said before connecting.
fn check_account(login: &Login) -> Result<(), &'static str> {
    if login.name.trim().is_empty() {
        return Err("Indique le nom du compte.");
    }
    if let Auth::Secret(s) = &login.auth
        && s.len() < 8
    {
        return Err("Le mot de passe a au moins 8 caractères.");
    }
    Ok(())
}

/// When the server closes the connection: the reason, and a button to quit.
pub fn disconnected_ui(
    mut contexts: EguiContexts,
    remote: Option<bevy::prelude::Res<Remote>>,
    mut god: ResMut<crate::god::God>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(remote) = remote else { return };
    let Some(reason) = remote
        .closed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    // Give the mouse back.
    god.on = false;
    egui::Window::new("Déconnecté")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!("Le serveur a fermé la connexion : {reason}"));
            if ui.button("Quitter").clicked() {
                exit.write(AppExit::Success);
            }
        });
}
