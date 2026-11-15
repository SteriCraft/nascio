//! `sun-curves`: sun elevation over one game day for several seam distances (M0 validation).

use std::path::Path;

use nascio_core::astro::AstroConfig;
use nascio_core::geo::{z_north, z_south};
use nascio_core::time::GameTime;

use crate::plot::{Canvas, hex, text_height, text_width};

/// Which side of the seam the curves are computed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hemisphere {
    North,
    South,
}

/// One sampled curve: local solar hour → (e, e_vis), both in degrees.
pub struct Curve {
    pub s: f64,
    pub samples: Vec<(f64, f64, f64)>,
}

/// Samples a game day `day` (integer day of year) at world x = 0, for each seam distance.
/// `steps` samples per day; the hour axis is the local solar time at x = 0.
pub fn sample(
    cfg: &AstroConfig,
    day: u32,
    seam_distances: &[f64],
    hemisphere: Hemisphere,
    steps: u32,
) -> Vec<Curve> {
    seam_distances
        .iter()
        .map(|&s| {
            let z = match hemisphere {
                Hemisphere::North => z_north(s),
                Hemisphere::South => z_south(s),
            };
            let samples = (0..=steps)
                .map(|i| {
                    let frac = f64::from(i) / f64::from(steps);
                    let t = GameTime::from_year_day(0, f64::from(day) + frac);
                    let sun = cfg.sun(t, 0.0, z);
                    (
                        24.0 * frac,
                        sun.elevation.to_degrees(),
                        sun.visible_elevation.to_degrees(),
                    )
                })
                .collect();
            Curve { s, samples }
        })
        .collect()
}

/// Prints an hourly table of `e / e_vis` in degrees.
pub fn print_table(day: u32, curves: &[Curve], steps: u32) {
    println!("day {day}: e / e_vis (degrees) by local solar hour at x = 0");
    print!("{:>6}", "hour");
    for c in curves {
        print!(" {:>15}", format!("s={} m", c.s));
    }
    println!();
    let per_hour = (steps / 24).max(1) as usize;
    for h in (0..=24).step_by(2) {
        print!("{h:>6}");
        for c in curves {
            let (_, e, v) = c.samples[(h * per_hour).min(c.samples.len() - 1)];
            print!(" {:>15}", format!("{e:6.1} /{v:6.1}"));
        }
        println!();
    }
    println!();
}

/// Default categorical order (dataviz reference palette, light mode).
const SERIES: [&str; 8] = [
    "#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948",
];
const SURFACE: &str = "#fcfcfb";
const TEXT: &str = "#0b0b0b";
const TEXT_2: &str = "#52514e";
const GRID: &str = "#e4e3df";

const W: i32 = 1400;
const PANEL_H: i32 = 420;
const LEFT: i32 = 80;
const RIGHT: i32 = 130;
const HEADER: i32 = 96;
const PANEL_TOP: i32 = 34;
const PANEL_BOTTOM: i32 = 40;
const E_MIN: f64 = -90.0;
const E_MAX: f64 = 90.0;

/// Renders one panel per day, stacked vertically, to a PNG.
pub fn render(
    panels: &[(String, Vec<Curve>)],
    hemisphere: Hemisphere,
    out: &Path,
) -> anyhow::Result<()> {
    let n = panels.len() as i32;
    let height = HEADER + n * PANEL_H;
    let mut cv = Canvas::new(W as u32, height as u32, hex(SURFACE));
    let (text, text2, grid) = (hex(TEXT), hex(TEXT_2), hex(GRID));

    let side = match hemisphere {
        Hemisphere::North => "north side (z = s)",
        Hemisphere::South => "south side (z = 100000 - s)",
    };
    cv.text(
        LEFT,
        16,
        &format!("Sun elevation over one day, x = 0, {side}"),
        2,
        text,
    );
    cv.text(
        LEFT,
        40,
        "solid: e (geometric)   dashed: e_vis (visible)",
        2,
        text2,
    );

    // Legend row (identity never by colour alone: each curve is also labelled at its end).
    let mut lx = LEFT;
    if let Some((_, curves)) = panels.first() {
        for (i, c) in curves.iter().enumerate() {
            let col = hex(SERIES[i % SERIES.len()]);
            cv.rect(lx, 70, 18, 4, col);
            let label = format!("s={} m", c.s);
            cv.text(lx + 24, 66, &label, 2, text2);
            lx += 24 + text_width(&label, 2) + 28;
        }
    }

    for (p, (title, curves)) in panels.iter().enumerate() {
        let top = HEADER + p as i32 * PANEL_H;
        let (x0, x1) = (LEFT, W - RIGHT);
        let (y0, y1) = (top + PANEL_TOP, top + PANEL_H - PANEL_BOTTOM);
        let px = |h: f64| f64::from(x0) + h / 24.0 * f64::from(x1 - x0);
        let py = |e: f64| f64::from(y1) - (e - E_MIN) / (E_MAX - E_MIN) * f64::from(y1 - y0);

        cv.text(x0, top + 8, title, 2, text);

        // Recessive grid: every 30° and every 3 h; the horizon is emphasised.
        for k in 0..=6 {
            let e = E_MIN + 30.0 * f64::from(k);
            let y = py(e).round() as i32;
            let (c, t) = if e == 0.0 { (text2, 2) } else { (grid, 1) };
            cv.hline(x0, x1, y, t, c);
            let label = format!("{e:+.0}°").replace("+0", "0");
            cv.text(
                x0 - 10 - text_width(&label, 2),
                y - text_height(2) / 2,
                &label,
                2,
                text2,
            );
        }
        for h in (0..=24).step_by(3) {
            let x = px(f64::from(h)).round() as i32;
            cv.vline(x, y0, y1, 1, grid);
            let label = format!("{h}h");
            cv.text(x - text_width(&label, 2) / 2, y1 + 10, &label, 2, text2);
        }

        // Curves.
        let mut end_labels = Vec::new();
        for (i, c) in curves.iter().enumerate() {
            let col = hex(SERIES[i % SERIES.len()]);
            let solid: Vec<_> = c.samples.iter().map(|&(h, e, _)| (px(h), py(e))).collect();
            cv.polyline(&solid, 2, col, None);
            let differs = c.samples.iter().any(|&(_, e, v)| (e - v).abs() > 1e-9);
            if differs {
                let vis: Vec<_> = c.samples.iter().map(|&(h, _, v)| (px(h), py(v))).collect();
                cv.polyline(&vis, 2, col, Some((10.0, 7.0)));
            }
            let &(_, _, v_end) = c.samples.last().expect("non-empty curve");
            end_labels.push((py(v_end), format!("s={}", c.s), col));
        }

        // Direct labels at the right end, pushed apart to avoid collisions.
        end_labels.sort_by(|a, b| a.0.total_cmp(&b.0));
        let gap = f64::from(text_height(2) + 4);
        let mut last = f64::NEG_INFINITY;
        for (y, label, col) in &mut end_labels {
            *y = y.max(last + gap);
            last = *y;
            let yi = y.round() as i32;
            cv.rect(x1 + 8, yi - 2, 10, 4, *col);
            cv.text(x1 + 22, yi - text_height(2) / 2, label, 2, text2);
        }
    }

    cv.save(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seam_curve_is_flat_and_hidden() {
        let curves = sample(
            &AstroConfig::default(),
            172,
            &[0.0, 50_000.0],
            Hemisphere::South,
            96,
        );
        assert_eq!(curves[0].samples.len(), 97);
        for &(_, e, v) in &curves[0].samples {
            assert!(e.abs() < 1e-6);
            assert!((v + 4.0).abs() < 1e-6);
        }
        // Equator: noon elevation at the June solstice is 90° − 23.44°.
        let noon = curves[1].samples[48];
        assert!((noon.1 - 66.56).abs() < 0.05, "{noon:?}");
    }
}
