//! Minimal raster drawing for debug PNGs (lines, rectangles, a built-in 5×7 bitmap font).
//!
//! Kept dependency-free on purpose: it only serves development previews in `out/`.

// Rendering only (colours, previews): platform maths is fine here.
#![allow(clippy::disallowed_methods)]

use image::{Rgb, RgbImage};

/// An RGB colour.
pub type Color = Rgb<u8>;

/// Colour from its components.
pub fn rgb(c: [u8; 3]) -> Color {
    Rgb(c)
}

/// Parses `#rrggbb`.
pub fn hex(s: &str) -> Color {
    let s = s.trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0);
    Rgb([p(0), p(2), p(4)])
}

/// A drawing surface.
pub struct Canvas {
    pub img: RgbImage,
}

impl Canvas {
    /// New canvas filled with `bg`.
    pub fn new(width: u32, height: u32, bg: Color) -> Self {
        Self {
            img: RgbImage::from_pixel(width, height, bg),
        }
    }

    /// Sets one pixel, ignoring out-of-bounds coordinates.
    pub fn put(&mut self, x: i32, y: i32, c: Color) {
        if x >= 0 && y >= 0 && (x as u32) < self.img.width() && (y as u32) < self.img.height() {
            self.img.put_pixel(x as u32, y as u32, c);
        }
    }

    /// Filled axis-aligned rectangle.
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, c);
            }
        }
    }

    /// Horizontal line of thickness `t`.
    pub fn hline(&mut self, x0: i32, x1: i32, y: i32, t: i32, c: Color) {
        self.rect(x0.min(x1), y, (x1 - x0).abs() + 1, t, c);
    }

    /// Vertical line of thickness `t`.
    pub fn vline(&mut self, x: i32, y0: i32, y1: i32, t: i32, c: Color) {
        self.rect(x, y0.min(y1), t, (y1 - y0).abs() + 1, c);
    }

    /// Polyline of thickness `t`. With `dash = Some((on, off))`, draws a dashed line whose
    /// pattern follows the arc length in pixels.
    pub fn polyline(&mut self, pts: &[(f64, f64)], t: i32, c: Color, dash: Option<(f64, f64)>) {
        let mut arc = 0.0;
        for w in pts.windows(2) {
            let (x0, y0) = w[0];
            let (x1, y1) = w[1];
            let len = (x1 - x0).hypot(y1 - y0);
            let steps = (len * 2.0).ceil().max(1.0) as i32;
            for i in 0..=steps {
                let f = f64::from(i) / f64::from(steps);
                let a = arc + f * len;
                if let Some((on, off)) = dash
                    && a.rem_euclid(on + off) >= on
                {
                    continue;
                }
                let x = (x0 + f * (x1 - x0)).round() as i32;
                let y = (y0 + f * (y1 - y0)).round() as i32;
                let o = t / 2;
                self.rect(x - o, y - o, t, t, c);
            }
            arc += len;
        }
    }

    /// Draws text with the built-in font at integer `scale`; `(x, y)` is the top-left corner.
    pub fn text(&mut self, x: i32, y: i32, s: &str, scale: i32, c: Color) {
        let mut cx = x;
        for ch in s.chars() {
            if let Some(rows) = glyph(ch) {
                for (ry, row) in rows.iter().enumerate() {
                    for (rx, b) in row.bytes().enumerate() {
                        if b == b'#' {
                            self.rect(
                                cx + rx as i32 * scale,
                                y + ry as i32 * scale,
                                scale,
                                scale,
                                c,
                            );
                        }
                    }
                }
            }
            cx += text_advance(scale);
        }
    }

    /// Saves as PNG, creating parent directories.
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.img.save(path)?;
        Ok(())
    }
}

/// Horizontal advance of one character at `scale`.
pub fn text_advance(scale: i32) -> i32 {
    6 * scale
}

/// Width in pixels of `s` at `scale`.
pub fn text_width(s: &str, scale: i32) -> i32 {
    s.chars().count() as i32 * text_advance(scale) - scale
}

/// Height in pixels of a line of text at `scale`.
pub fn text_height(scale: i32) -> i32 {
    7 * scale
}

#[rustfmt::skip]
fn glyph(c: char) -> Option<[&'static str; 7]> {
    Some(match c {
        '0' => [" ### ", "#   #", "#  ##", "# # #", "##  #", "#   #", " ### "],
        '1' => ["  #  ", " ##  ", "  #  ", "  #  ", "  #  ", "  #  ", " ### "],
        '2' => [" ### ", "#   #", "    #", "   # ", "  #  ", " #   ", "#####"],
        '3' => ["#####", "   # ", "  #  ", "   # ", "    #", "#   #", " ### "],
        '4' => ["   # ", "  ## ", " # # ", "#  # ", "#####", "   # ", "   # "],
        '5' => ["#####", "#    ", "#### ", "    #", "    #", "#   #", " ### "],
        '6' => ["  ## ", " #   ", "#    ", "#### ", "#   #", "#   #", " ### "],
        '7' => ["#####", "    #", "   # ", "  #  ", " #   ", " #   ", " #   "],
        '8' => [" ### ", "#   #", "#   #", " ### ", "#   #", "#   #", " ### "],
        '9' => [" ### ", "#   #", "#   #", " ####", "    #", "   # ", " ##  "],
        '-' => ["     ", "     ", "     ", "#####", "     ", "     ", "     "],
        '+' => ["     ", "  #  ", "  #  ", "#####", "  #  ", "  #  ", "     "],
        '.' => ["     ", "     ", "     ", "     ", "     ", " ##  ", " ##  "],
        '%' => ["##  #", "##  #", "   # ", "  #  ", " #   ", "#  ##", "#  ##"],
        ',' => ["     ", "     ", "     ", "     ", " ##  ", "  #  ", " #   "],
        '=' => ["     ", "     ", "#####", "     ", "#####", "     ", "     "],
        ':' => ["     ", " ##  ", " ##  ", "     ", " ##  ", " ##  ", "     "],
        '_' => ["     ", "     ", "     ", "     ", "     ", "     ", "#####"],
        '(' => ["   # ", "  #  ", " #   ", " #   ", " #   ", "  #  ", "   # "],
        ')' => [" #   ", "  #  ", "   # ", "   # ", "   # ", "  #  ", " #   "],
        '/' => ["     ", "    #", "   # ", "  #  ", " #   ", "#    ", "     "],
        '°' => [" ##  ", "#  # ", "#  # ", " ##  ", "     ", "     ", "     "],
        'a' => ["     ", "     ", " ### ", "    #", " ####", "#   #", " ####"],
        'b' => ["#    ", "#    ", "# ## ", "##  #", "#   #", "#   #", "#### "],
        'c' => ["     ", "     ", " ### ", "#    ", "#    ", "#   #", " ### "],
        'd' => ["    #", "    #", " ## #", "#  ##", "#   #", "#   #", " ####"],
        'e' => ["     ", "     ", " ### ", "#   #", "#####", "#    ", " ### "],
        'f' => ["  ## ", " #   ", "#### ", " #   ", " #   ", " #   ", " #   "],
        'k' => ["#    ", "#    ", "#  # ", "# #  ", "##   ", "# #  ", "#  # "],
        'w' => ["     ", "     ", "#   #", "#   #", "# # #", "# # #", " # # "],
        'g' => ["     ", "     ", " ####", "#   #", " ####", "    #", " ### "],
        'h' => ["#    ", "#    ", "# ## ", "##  #", "#   #", "#   #", "#   #"],
        'i' => ["  #  ", "     ", " ##  ", "  #  ", "  #  ", "  #  ", " ### "],
        'l' => [" ##  ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", " ### "],
        'm' => ["     ", "     ", "## # ", "# # #", "# # #", "#   #", "#   #"],
        'n' => ["     ", "     ", "# ## ", "##  #", "#   #", "#   #", "#   #"],
        'o' => ["     ", "     ", " ### ", "#   #", "#   #", "#   #", " ### "],
        'p' => ["     ", "     ", "#### ", "#   #", "#### ", "#    ", "#    "],
        'r' => ["     ", "     ", "# ## ", "##  #", "#    ", "#    ", "#    "],
        's' => ["     ", "     ", " ####", "#    ", " ### ", "    #", "#### "],
        't' => [" #   ", " #   ", "###  ", " #   ", " #   ", " #  #", "  ## "],
        'u' => ["     ", "     ", "#   #", "#   #", "#   #", "#  ##", " ## #"],
        'v' => ["     ", "     ", "#   #", "#   #", "#   #", " # # ", "  #  "],
        'x' => ["     ", "     ", "#   #", " # # ", "  #  ", " # # ", "#   #"],
        'y' => ["     ", "     ", "#   #", "#   #", " ####", "    #", " ### "],
        'z' => ["     ", "     ", "#####", "   # ", "  #  ", " #   ", "#####"],
        'C' => [" ### ", "#   #", "#    ", "#    ", "#    ", "#   #", " ### "],
        'j' => ["   # ", "     ", "  ## ", "   # ", "   # ", "#  # ", " ##  "],
        'q' => ["     ", "     ", " ####", "#   #", " ####", "    #", "    #"],
        'D' => ["#### ", "#   #", "#   #", "#   #", "#   #", "#   #", "#### "],
        'J' => ["  ###", "   # ", "   # ", "   # ", "   # ", "#  # ", " ##  "],
        'M' => ["#   #", "## ##", "# # #", "# # #", "#   #", "#   #", "#   #"],
        'N' => ["#   #", "##  #", "# # #", "#  ##", "#   #", "#   #", "#   #"],
        'Q' => [" ### ", "#   #", "#   #", "#   #", "# # #", "#  # ", " ## #"],
        'S' => [" ####", "#    ", "#    ", " ### ", "    #", "    #", "#### "],
        _ => return None,
    })
}
