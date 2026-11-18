//! Measurements of the server over the last minutes (what the terminal prints every 10 s),
//! sampled once a second and drawn as small line charts.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use bevy_egui::egui;
use nascio_server::net::Shared;
use nascio_server::sim::SimStats;

/// Samples kept: five minutes.
const KEPT: usize = 300;
/// Height of a chart, points.
const HEIGHT: f32 = 64.0;

/// Colours of the series (light enough on the dark theme).
const BLUE: egui::Color32 = egui::Color32::from_rgb(90, 160, 240);
const ORANGE: egui::Color32 = egui::Color32::from_rgb(240, 140, 60);
const GREEN: egui::Color32 = egui::Color32::from_rgb(80, 200, 130);

#[derive(Default)]
pub struct History {
    /// Time of the last sample, s (window clock).
    last: Option<f64>,
    /// Counters at the last sample: column bytes sent, entity bytes, columns built, edits.
    counters: [u64; 4],
    tick_p50: VecDeque<f32>,
    tick_p99: VecDeque<f32>,
    players: VecDeque<f32>,
    columns_kib_s: VecDeque<f32>,
    entities_kib_s: VecDeque<f32>,
    built_s: VecDeque<f32>,
    edits_s: VecDeque<f32>,
}

fn push(v: &mut VecDeque<f32>, x: f64) {
    v.push_back(x as f32);
    if v.len() > KEPT {
        v.pop_front();
    }
}

impl History {
    /// Takes a sample if a second went by since the last.
    pub fn update(&mut self, now: f64, shared: &Shared, stats: &Mutex<SimStats>) {
        if self.last.is_some_and(|t| now - t < 1.0) {
            return;
        }
        let s = stats.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let counters = [
            shared.sent_bytes.load(Ordering::Relaxed),
            s.entity_bytes,
            shared.cache.built.load(Ordering::Relaxed),
            s.edits,
        ];
        if let Some(t) = self.last {
            let dt = (now - t).max(1e-3);
            let rate = |k: usize| counters[k].saturating_sub(self.counters[k]) as f64 / dt;
            push(&mut self.tick_p50, s.tick_ms(0.5));
            push(&mut self.tick_p99, s.tick_ms(0.99));
            push(&mut self.players, s.players as f64);
            push(&mut self.columns_kib_s, rate(0) / 1_024.0);
            push(&mut self.entities_kib_s, rate(1) / 1_024.0);
            push(&mut self.built_s, rate(2));
            push(&mut self.edits_s, rate(3));
        }
        self.last = Some(now);
        self.counters = counters;
    }
}

/// The charts, one under the other.
pub fn show(ui: &mut egui::Ui, h: &History) {
    chart(
        ui,
        "Durée du tick (ms, budget 50)",
        &[("p50", BLUE, &h.tick_p50), ("p99", ORANGE, &h.tick_p99)],
        Some(50.0),
    );
    chart(ui, "Joueurs", &[("", GREEN, &h.players)], None);
    chart(
        ui,
        "Envoi (Kio/s)",
        &[
            ("colonnes", BLUE, &h.columns_kib_s),
            ("entités", ORANGE, &h.entities_kib_s),
        ],
        None,
    );
    chart(
        ui,
        "Par seconde",
        &[
            ("colonnes générées", BLUE, &h.built_s),
            ("éditions", ORANGE, &h.edits_s),
        ],
        None,
    );
}

/// A line chart of the last [`KEPT`] samples: its title, the last values, the scale from 0
/// to the highest value (or `budget`, drawn dashed, if higher).
fn chart(
    ui: &mut egui::Ui,
    title: &str,
    series: &[(&str, egui::Color32, &VecDeque<f32>)],
    budget: Option<f32>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(title);
        for (name, color, v) in series {
            let last = v.back().copied().unwrap_or(0.0);
            let text = if name.is_empty() {
                format!("{last:.0}")
            } else {
                format!("{name} {last:.1}")
            };
            ui.colored_label(*color, text);
        }
    });
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEIGHT),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(18));
    let top = series
        .iter()
        .flat_map(|(_, _, v)| v.iter().copied())
        .fold(budget.unwrap_or(0.0), f32::max)
        .max(1.0)
        * 1.1;
    let y = |v: f32| rect.bottom() - (v / top) * rect.height();
    let x = |k: usize| rect.left() + rect.width() * k as f32 / (KEPT - 1) as f32;
    if let Some(b) = budget {
        let stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(110));
        painter.add(egui::Shape::dashed_line(
            &[
                egui::pos2(rect.left(), y(b)),
                egui::pos2(rect.right(), y(b)),
            ],
            stroke,
            4.0,
            4.0,
        ));
    }
    painter.text(
        rect.left_top() + egui::vec2(3.0, 2.0),
        egui::Align2::LEFT_TOP,
        format!("{:.0}", top / 1.1),
        egui::FontId::proportional(10.0),
        egui::Color32::from_gray(140),
    );
    for (_, color, v) in series {
        // The newest sample on the right edge.
        let start = KEPT - v.len();
        let points: Vec<egui::Pos2> = v
            .iter()
            .enumerate()
            .map(|(k, &val)| egui::pos2(x(start + k), y(val)))
            .collect();
        if points.len() >= 2 {
            painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, *color)));
        }
    }
    ui.add_space(4.0);
}
