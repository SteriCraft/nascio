//! The chat (network mode): Enter opens a line to write to every player; the lines of the
//! server (arrivals, departures, bans, players, administrators) show in the bottom-left
//! corner and fade after a while when the chat is closed.

use std::collections::VecDeque;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use nascio_protocol::messages::{Chat, ClientMsg, MAX_CHAT_CHARS};

use crate::remote::Remote;

/// Lines kept.
const KEPT: usize = 100;
/// Lines shown while the chat is open.
const SHOWN_OPEN: usize = 14;
/// A line stays this long when the chat is closed, then fades for one more second (s).
const STAYS_S: f64 = 10.0;

#[derive(Resource, Default)]
pub struct ChatBox {
    /// The line to write is open: the keyboard belongs to it.
    pub open: bool,
    input: String,
    /// Text, colour and arrival time (s) of the lines.
    lines: VecDeque<(String, egui::Color32, f64)>,
}

/// How a line of the server reads.
fn wording(line: Chat) -> (String, egui::Color32) {
    let event = egui::Color32::from_rgb(255, 225, 120);
    let admin = egui::Color32::from_rgb(255, 120, 90);
    match line {
        Chat::Joined(n) => (format!("{n} a rejoint le serveur"), event),
        Chat::Left(n) => (format!("{n} a quitté le serveur"), event),
        Chat::Banned(n) => (format!("{n} a été banni"), event),
        Chat::Say { from, text } => (format!("<{from}> {text}"), egui::Color32::WHITE),
        Chat::Admin(text) => (format!("[Admin]: {text}"), admin),
    }
}

/// Takes the new lines, opens the chat on Enter, sends the line written.
pub fn chat_ui(
    mut contexts: EguiContexts,
    mut chat: ResMut<ChatBox>,
    remote: Option<Res<Remote>>,
    time: Res<Time<Real>>,
    pause: Res<crate::pause::Pause>,
) {
    let Some(remote) = remote else { return };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let chat = &mut *chat;
    for line in std::mem::take(&mut *remote.chat.lock().unwrap_or_else(|e| e.into_inner())) {
        let (text, color) = wording(line);
        chat.lines.push_back((text, color, now));
        if chat.lines.len() > KEPT {
            chat.lines.pop_front();
        }
    }
    if !chat.open
        && !pause.open
        && !ctx.egui_wants_keyboard_input()
        && ctx.input(|i| i.key_pressed(egui::Key::Enter))
    {
        chat.open = true;
    }
    let open = chat.open;
    let shown: Vec<&(String, egui::Color32, f64)> = if open {
        chat.lines.iter().rev().take(SHOWN_OPEN).rev().collect()
    } else {
        chat.lines
            .iter()
            .filter(|l| now - l.2 < STAYS_S + 1.0)
            .collect()
    };
    if shown.is_empty() && !open {
        return;
    }
    let mut send = None;
    let mut close = false;
    egui::Area::new(egui::Id::new("chat"))
        .anchor(egui::Align2::LEFT_BOTTOM, [10.0, -10.0])
        .show(ctx, |ui| {
            ui.set_max_width(520.0);
            let frame = egui::Frame::NONE
                .fill(egui::Color32::from_black_alpha(if open { 150 } else { 90 }))
                .corner_radius(4.0)
                .inner_margin(6.0);
            frame.show(ui, |ui| {
                for (text, color, at) in &shown {
                    // Closed: the line fades out during its last second.
                    let alpha = if open {
                        1.0
                    } else {
                        (STAYS_S + 1.0 - (now - at)).clamp(0.0, 1.0) as f32
                    };
                    ui.label(egui::RichText::new(text).color(color.gamma_multiply(alpha)));
                }
                if open {
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut chat.input)
                            .char_limit(MAX_CHAT_CHARS)
                            .hint_text("Entrée : envoyer · Échap : fermer")
                            .desired_width(500.0),
                    );
                    if !edit.has_focus() && !edit.lost_focus() {
                        edit.request_focus();
                    }
                    let (enter, escape) = ui.input(|i| {
                        (
                            i.key_pressed(egui::Key::Enter),
                            i.key_pressed(egui::Key::Escape),
                        )
                    });
                    if escape {
                        close = true;
                    } else if enter && edit.lost_focus() {
                        send = Some(std::mem::take(&mut chat.input));
                        close = true;
                    }
                }
            });
        });
    if let Some(text) = send
        && !text.trim().is_empty()
    {
        remote.send(ClientMsg::Chat {
            text: text.trim().to_owned(),
        });
    }
    if close {
        chat.open = false;
        chat.input.clear();
    }
    if !open && !shown.is_empty() {
        // Let the lines fade without input.
        ctx.request_repaint();
    }
}
