//! The look of the menus, taken from the author's former project (a Unity scene): white
//! background, "NASCIO" title in Stentiga, peach buttons turning orange under the mouse, dark
//! grey text in Prototype (author's decision, `docs/decisions.md`).
//!
//! It dresses the main menu, the connection, settings, pause, disconnection and death windows,
//! and nothing else: the chat, the HUD and the development panels keep egui's default style.
//! The fonts are registered as named families, so the default ones are untouched.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::prelude::{Local, Result};
use bevy_egui::{EguiContexts, egui};
use egui::{Color32, FontFamily, FontId, Stroke, TextStyle};

/// Title font (`assets/fonts/stentiga.ttf`, see `ASSETS.md`).
pub const TITLE: &str = "stentiga";
/// Text font (`assets/fonts/prototype.ttf`, see `ASSETS.md`).
pub const TEXT: &str = "prototype";

/// Text, #323232.
pub const INK: Color32 = Color32::from_rgb(50, 50, 50);
/// Background.
pub const PAPER: Color32 = Color32::WHITE;
/// Button at rest, #FFCCA0.
pub const PEACH: Color32 = Color32::from_rgb(255, 204, 160);
/// Button under the mouse, #FFA353.
pub const HOVER: Color32 = Color32::from_rgb(255, 163, 83);
/// Button pressed, #FF8519.
pub const PRESSED: Color32 = Color32::from_rgb(255, 133, 25);

/// The layout of the main menu is given for this window width, and scales with it (as the
/// Unity canvas did).
pub const REF_WIDTH: f32 = 1280.0;
/// Size of the big buttons, and the step between their centres, at [`REF_WIDTH`].
pub const BUTTON: egui::Vec2 = egui::vec2(350.0, 40.0);
pub const BUTTON_STEP: f32 = 70.0;
/// Text size of the big buttons, of the text of the windows.
pub const BUTTON_TEXT: f32 = 22.0;
const BODY_TEXT: f32 = 18.0;

/// The named families exist: egui takes new fonts at the next pass, and asking for a family
/// it does not know panics.
static READY: AtomicBool = AtomicBool::new(false);

fn family(name: &str) -> FontFamily {
    if READY.load(Ordering::Relaxed) {
        FontFamily::Name(name.into())
    } else {
        FontFamily::Proportional
    }
}

pub fn title_family() -> FontFamily {
    family(TITLE)
}

pub fn text_family() -> FontFamily {
    family(TEXT)
}

/// Registers the two fonts once, as named families (with the default fonts behind them for
/// any missing glyph); they are used from the next pass on.
pub fn install_fonts(mut contexts: EguiContexts, mut done: Local<bool>) -> Result {
    if *done {
        READY.store(true, Ordering::Relaxed);
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let mut fonts = egui::FontDefinitions::default();
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (name, bytes) in [
        (TITLE, &include_bytes!("../assets/fonts/stentiga.ttf")[..]),
        (TEXT, &include_bytes!("../assets/fonts/prototype.ttf")[..]),
    ] {
        fonts
            .font_data
            .insert(name.into(), Arc::new(egui::FontData::from_static(bytes)));
        let mut family = vec![name.to_owned()];
        family.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(name.into()), family);
    }
    ctx.set_fonts(fonts);
    *done = true;
    Ok(())
}

/// The style of the menus.
pub fn style() -> egui::Style {
    let mut s = egui::Style {
        visuals: egui::Visuals::light(),
        ..Default::default()
    };
    let text = text_family();
    for (style, size) in [
        (TextStyle::Body, BODY_TEXT),
        (TextStyle::Button, BODY_TEXT),
        (TextStyle::Heading, 26.0),
        (TextStyle::Small, 14.0),
    ] {
        s.text_styles.insert(style, FontId::new(size, text.clone()));
    }
    s.spacing.button_padding = egui::vec2(10.0, 5.0);
    s.spacing.item_spacing = egui::vec2(8.0, 8.0);

    let v = &mut s.visuals;
    v.override_text_color = Some(INK);
    v.panel_fill = PAPER;
    v.window_fill = PAPER;
    v.window_stroke = Stroke::new(1.5, PEACH);
    v.window_corner_radius = egui::CornerRadius::same(6);
    v.extreme_bg_color = Color32::from_rgb(255, 248, 242);
    v.selection.bg_fill = HOVER.gamma_multiply(0.6);
    v.selection.stroke = Stroke::new(1.0, INK);
    let w = &mut v.widgets;
    for (state, fill) in [
        (&mut w.inactive, PEACH),
        (&mut w.hovered, HOVER),
        (&mut w.active, PRESSED),
        (&mut w.open, HOVER),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        // Invisible on a button, a thin border around a text field.
        state.bg_stroke = Stroke::new(1.0, fill);
        state.fg_stroke = Stroke::new(1.5, INK);
        state.corner_radius = egui::CornerRadius::same(4);
    }
    w.noninteractive.fg_stroke = Stroke::new(1.0, INK);
    w.noninteractive.bg_stroke = Stroke::new(1.0, PEACH);
    s
}

/// A big button of the menus, `scale` times its size at [`REF_WIDTH`].
pub fn big_button(text: &str, scale: f32) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(text)
            .family(text_family())
            .size(BUTTON_TEXT * scale),
    )
    .min_size(BUTTON * scale)
}

/// A window of the menus, centred with `offset`: the theme's frame, `title` (if any) and a
/// cross closing it (if `open` is given), then the contents.
pub fn dialog<R>(
    ctx: &egui::Context,
    id: &str,
    title: Option<&str>,
    offset: [f32; 2],
    open: Option<&mut bool>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    let style = style();
    let frame = egui::Frame::window(&style)
        .inner_margin(egui::Margin::same(16))
        .corner_radius(egui::CornerRadius::same(6));
    egui::Window::new(id)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .frame(frame)
        .anchor(egui::Align2::CENTER_CENTER, offset)
        .show(ctx, |ui| {
            ui.set_style(style);
            let top = ui.cursor().min.y;
            if let Some(t) = title {
                // Room for the cross on the title's line.
                ui.horizontal(|ui| {
                    ui.heading(t);
                    if open.is_some() {
                        ui.add_space(32.0);
                    }
                });
                ui.add_space(6.0);
            }
            let r = add_contents(ui);
            // The cross, top right, once the width is known (it does not widen the window).
            if let Some(open) = open {
                let right = ui.min_rect().right();
                let rect = egui::Rect::from_min_size(
                    egui::pos2(right - 26.0, top),
                    egui::vec2(26.0, 26.0),
                );
                if ui
                    .put(rect, egui::Button::new(egui::RichText::new("×").size(20.0)))
                    .clicked()
                {
                    *open = false;
                }
            }
            r
        })
        .and_then(|r| r.inner)
}
