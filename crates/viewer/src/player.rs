//! The player in network mode (chantier 5.5). The keyboard drives the player that
//! `nascio_client::Player` predicts, and the camera sits at its eyes; the server simulates the
//! same commands with the authority (`docs/world-spec.md` §8.4).
//!
//! - ZQSD (physical WASD) moves, Space jumps (swims up, flies up), Shift crouches (swims and
//!   flies down). Two presses of Z within 300 ms run; the view then widens from 42° to 60°.
//! - The mouse turns the view (captured while playing; Échap, the chat and death free it).
//! - Administrators: two presses of Space within 300 ms ask to fly, or to walk again; the view
//!   is at 60° while flying (author's decision). Other players never ask.
//! - Death: a grey veil with its cause and a "Réapparaître" button.
//!
//! The network thread gives the player the server's states, its death and respawn and the
//! round trip time ([`crate::remote`]); this module runs it once per frame.

use std::time::Instant;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use nascio_client::{Controls, PlayerEvent};
use nascio_core::coords::WorldPos;
use nascio_physics::Mode as Motion;
use nascio_protocol::messages::{ClientMsg, DeathCause};

use crate::fly::{FlyCam, FlyCamera};
use crate::remote::Remote;

/// Vertical field of view walking, and running, degrees (author's decision).
pub const FOV_WALK_DEG: f32 = 42.0;
pub const FOV_RUN_DEG: f32 = 60.0;
/// Time constant of the change of the field of view, s.
const FOV_TAU_S: f32 = 0.15;
/// Reach of the edits from the eyes, m (the server's `edit_reach_m`).
pub const EDIT_REACH_M: f64 = 5.0;
/// Two presses of Space within this time ask to fly or to walk (as running, s).
const FLY_TAP_S: f64 = 0.3;
/// A request to fly not followed by the mode within this time was refused, s.
const FLY_ANSWER_S: f64 = 2.0;
/// A message of the player stays this long, s.
const NOTICE_S: f64 = 4.0;

/// What the interface shows of the player.
#[derive(Resource, Default)]
pub struct Life {
    /// Dead, and why.
    pub died: Option<DeathCause>,
    /// "Réapparaître" was pressed: waiting for the server.
    respawn_asked: bool,
    pub running: bool,
    pub flying: bool,
    /// Mode asked and when (s), until the server's states show it.
    fly_asked: Option<(bool, f64)>,
    /// Last press of Space (s).
    space_tap: Option<f64>,
    /// A short message and when it came (s).
    notice: Option<(String, f64)>,
}

impl Life {
    /// The mouse turns the view: alive, connected.
    pub fn playing(&self) -> bool {
        self.died.is_none()
    }
}

/// Back to the menu: the next connection starts alive.
pub fn reset_life(mut life: ResMut<Life>) {
    *life = Life::default();
}

/// Once per frame, connected: the keys become the player's commands (sent to the server
/// every three steps), its events reach the interface, and the camera goes to its eyes.
#[allow(clippy::too_many_arguments)]
pub fn walk(
    remote: Option<Res<Remote>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    saved: Res<crate::settings::Saved>,
    chat: Res<crate::chat::ChatBox>,
    pause: Res<crate::pause::Pause>,
    mut cam: ResMut<FlyCam>,
    mut life: ResMut<Life>,
) {
    let Some(remote) = remote else { return };
    let now_s = time.elapsed_secs_f64();
    // Writing in the chat, in the menu or in a capture: no key reaches the player.
    let free = !(chat.open || pause.open || cam.scripted);
    let held = |k: KeyCode| free && keys.pressed(k);
    let controls = Controls {
        forward: held(KeyCode::KeyW),
        back: held(KeyCode::KeyS),
        left: held(KeyCode::KeyA),
        right: held(KeyCode::KeyD),
        jump: held(KeyCode::Space),
        crouch: held(KeyCode::ShiftLeft) || held(KeyCode::ShiftRight),
        yaw: cam.yaw,
        pitch: cam.pitch as f32,
        // The physics caps it at the server's `fly_max_speed`.
        fly_speed: (cam.speed * 10.0).round().min(f64::from(u16::MAX)) as u16,
    };
    let (events, eye, running, flying) = {
        let mut player = remote.lock_player();
        player.set_auto_jump(saved.auto_jump);
        let commands = player.update(Instant::now(), &controls, &remote.lock_mirror());
        if let Some(c) = commands {
            remote.send_commands(c);
        }
        (
            player.events(),
            player.eye(),
            player.running(),
            player.state().mode == Motion::Fly,
        )
    };
    for e in events {
        match e {
            PlayerEvent::Died(cause) => life.died = Some(cause),
            PlayerEvent::Respawned => {
                life.died = None;
                life.respawn_asked = false;
            }
            PlayerEvent::Mode(m) => {
                let text = match m {
                    Motion::Fly => "Vol",
                    Motion::Walk => "Marche",
                };
                life.notice = Some((text.into(), now_s));
            }
            PlayerEvent::Snapped => {}
        }
    }
    cam.pos = WorldPos::new(eye[0], eye[1], eye[2]);
    life.running = running;
    life.flying = flying;
    // Flight: two presses of Space, administrators only (the server checks again).
    if free && remote.admin && life.died.is_none() && keys.just_pressed(KeyCode::Space) {
        if life.space_tap.is_some_and(|t| now_s - t <= FLY_TAP_S) {
            remote.send(ClientMsg::Fly(!flying));
            life.fly_asked = Some((!flying, now_s));
            life.space_tap = None;
        } else {
            life.space_tap = Some(now_s);
        }
    }
    if let Some((want, at)) = life.fly_asked {
        if want == flying {
            life.fly_asked = None;
        } else if now_s - at > FLY_ANSWER_S {
            life.fly_asked = None;
            life.notice = Some(("Vol refusé par le serveur".into(), now_s));
        }
    }
}

/// The field of view: 60° running or flying, 42° otherwise, smoothly.
pub fn field_of_view(
    life: Res<Life>,
    remote: Option<Res<Remote>>,
    time: Res<Time<Real>>,
    mut cameras: Query<&mut Projection, With<FlyCamera>>,
) {
    let deg = if remote.is_some() && (life.running || life.flying) {
        FOV_RUN_DEG
    } else {
        FOV_WALK_DEG
    };
    let target = deg.to_radians();
    let k = 1.0 - (-time.delta_secs() / FOV_TAU_S).exp();
    for mut p in &mut cameras {
        let Projection::Perspective(pp) = p.as_ref() else {
            continue;
        };
        let gap = target - pp.fov;
        if gap.abs() < 1e-5 {
            continue;
        }
        if let Projection::Perspective(pp) = p.as_mut() {
            pp.fov = if gap.abs() < 1e-3 {
                target
            } else {
                pp.fov + gap * k
            };
        }
    }
}

/// Death: a grey veil, its cause and the button to respawn. A short notice of the mode.
pub fn life_ui(
    mut contexts: EguiContexts,
    remote: Option<Res<Remote>>,
    time: Res<Time<Real>>,
    mut life: ResMut<Life>,
) -> Result {
    let Some(remote) = remote else { return Ok(()) };
    let ctx = contexts.ctx_mut()?;
    let now_s = time.elapsed_secs_f64();
    if let Some((text, at)) = &life.notice {
        if now_s - at > NOTICE_S {
            life.notice = None;
        } else {
            egui::Area::new(egui::Id::new("player-notice"))
                .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(text.as_str()));
                });
        }
    }
    let Some(cause) = life.died else {
        return Ok(());
    };
    egui::Area::new(egui::Id::new("death-veil"))
        .order(egui::Order::Background)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            ui.painter().rect_filled(
                ctx.viewport_rect(),
                0.0,
                egui::Color32::from_rgba_unmultiplied(70, 70, 70, 170),
            );
        });
    let wording = match cause {
        DeathCause::Suffocation => "Mort par étouffement",
        DeathCause::Magma => "Mort dans le magma",
    };
    let mut respawn = false;
    egui::Window::new("Mort")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(wording).size(30.0).strong());
                ui.add_space(16.0);
                let button = egui::Button::new(egui::RichText::new("Réapparaître").size(18.0))
                    .min_size(egui::vec2(220.0, 38.0));
                respawn = ui.add_enabled(!life.respawn_asked, button).clicked();
            });
        });
    if respawn {
        remote.send(ClientMsg::Respawn);
        life.respawn_asked = true;
    }
    Ok(())
}
