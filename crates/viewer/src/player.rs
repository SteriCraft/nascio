//! The player in network mode (chantier 5.5). The keyboard drives the player that
//! `nascio_client::Player` predicts, and the camera sits at its eyes; the server simulates the
//! same commands with the authority (`docs/world-spec.md` §8.4).
//!
//! - ZQSD (physical WASD) moves, Space jumps (swims up, flies up), Shift crouches (swims and
//!   flies down). Two presses of Z within 300 ms run; the view then widens by 1.3 times the
//!   player's field of view (a setting, 60° by default).
//! - The mouse turns the view (captured while playing; Échap, the chat and death free it).
//! - God mode (administrators, [`crate::god`]): two presses of Space within 300 ms ask to fly,
//!   or to walk again; the view widens the same way while flying, at a fixed 20 m/s, and coming
//!   down onto a block walks again (author's decisions). In walk mode they do nothing.
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

/// Vertical field of view set by the player: default and slider range, degrees (author's
/// decision).
pub const FOV_DEFAULT_DEG: f32 = 60.0;
pub const FOV_MIN_DEG: f32 = 40.0;
pub const FOV_MAX_DEG: f32 = 100.0;
/// Running or flying, the view widens by this factor (author's decision).
pub const FOV_RUN_FACTOR: f32 = 1.3;
/// Time constant of the change of the field of view, s.
const FOV_TAU_S: f32 = 0.15;
/// Reach of the edits from the eyes, m (the server's `edit_reach_m`).
pub const EDIT_REACH_M: f64 = 5.0;
/// Two presses of Space within this time ask to fly or to walk (as running, s).
const FLY_TAP_S: f64 = 0.3;
/// Flight speed, m/s (author's decision: fixed, the wheel does nothing connected).
pub const FLY_SPEED_MPS: f64 = 20.0;

/// What the interface shows of the player.
#[derive(Resource, Default)]
pub struct Life {
    /// Dead, and why.
    pub died: Option<DeathCause>,
    /// "Réapparaître" was pressed: waiting for the server.
    respawn_asked: bool,
    pub running: bool,
    pub flying: bool,
    /// Last press of Space (s).
    space_tap: Option<f64>,
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
    god: Res<crate::god::God>,
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
        // In 0.1 m/s; the physics caps it at the server's `fly_max_speed`.
        fly_speed: (FLY_SPEED_MPS * 10.0).round() as u16,
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
            // No message on the change of mode (author's decision).
            PlayerEvent::Mode(_) | PlayerEvent::Snapped => {}
        }
    }
    cam.pos = WorldPos::new(eye[0], eye[1], eye[2]);
    life.running = running;
    life.flying = flying;
    // Flight: two presses of Space, in god mode only (the server checks again).
    if free && god.on && life.died.is_none() && keys.just_pressed(KeyCode::Space) {
        if life.space_tap.is_some_and(|t| now_s - t <= FLY_TAP_S) {
            remote.send(ClientMsg::Fly(!flying));
            life.space_tap = None;
        } else {
            life.space_tap = Some(now_s);
        }
    }
}

/// The field of view: the player's, 1.3 times wider running or flying, smoothly.
pub fn field_of_view(
    life: Res<Life>,
    remote: Option<Res<Remote>>,
    saved: Res<crate::settings::Saved>,
    time: Res<Time<Real>>,
    mut cameras: Query<&mut Projection, With<FlyCamera>>,
) {
    let base = saved.fov_deg.clamp(FOV_MIN_DEG, FOV_MAX_DEG);
    let deg = if remote.is_some() && (life.running || life.flying) {
        base * FOV_RUN_FACTOR
    } else {
        base
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

/// Death: a grey veil, its cause and the button to respawn.
pub fn life_ui(
    mut contexts: EguiContexts,
    remote: Option<Res<Remote>>,
    mut life: ResMut<Life>,
) -> Result {
    let Some(remote) = remote else { return Ok(()) };
    let ctx = contexts.ctx_mut()?;
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
    crate::theme::dialog(ctx, "death", None, [0.0, 0.0], None, |ui| {
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(wording).size(30.0));
            ui.add_space(16.0);
            let button = crate::theme::big_button("Réapparaître", 1.0);
            respawn = ui.add_enabled(!life.respawn_asked, button).clicked();
        });
    });
    if respawn {
        remote.send(ClientMsg::Respawn);
        life.respawn_asked = true;
    }
    Ok(())
}
