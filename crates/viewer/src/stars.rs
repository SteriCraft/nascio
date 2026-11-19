//! Star catalogue (spec §2.8: real stars, equatorial coordinates J2000).
//!
//! The stars come from the Yale Bright Star Catalogue (5th revised edition, Hoffleit & Warren
//! 1991) down to magnitude 6.5, the absolute limit of the display (the catalogue goes to 7.96):
//! 8 404 stars in `assets/stars/bsc5.csv`, read at start-up (see `ASSETS.md`). Their colour follows
//! the B−V index; a slider sets the faintest shown.

use std::path::{Path, PathBuf};

use anyhow::Context;

/// Catalogue shipped with the viewer.
pub const CATALOGUE: &str = "assets/stars/bsc5.csv";
/// Absolute magnitude limit of the display (end of the limit slider); the shipped catalogue
/// stops there.
pub const FAINTEST: f64 = 6.5;
/// Default magnitude limit.
pub const DEFAULT_LIMIT: f64 = FAINTEST;
/// Brightest end of the limit slider: Sirius (−1.46) and Canopus (−0.72) stay visible.
pub const SLIDER_BRIGHTEST: f64 = -1.0;

/// A star: equatorial position (J2000), visual magnitude and B−V colour index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Star {
    /// Right ascension, rad.
    pub ra: f64,
    /// Declination, rad.
    pub dec: f64,
    pub vmag: f64,
    /// B−V, mag (0.6, a sun-like white, when the catalogue has none).
    pub bv: f64,
}

/// Parses the CSV catalogue (`#` comments, a header line, then
/// `hr,name,ra_deg,dec_deg,vmag,bv`).
pub fn parse(text: &str) -> anyhow::Result<Vec<Star>> {
    let mut stars = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("hr,") {
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        anyhow::ensure!(f.len() == 6, "line {}: expected 6 fields", n + 1);
        let num = |s: &str| -> anyhow::Result<f64> {
            s.trim()
                .trim_start_matches('+')
                .parse::<f64>()
                .with_context(|| format!("line {}: bad number {s:?}", n + 1))
        };
        stars.push(Star {
            ra: num(f[2])?.to_radians(),
            dec: num(f[3])?.to_radians(),
            vmag: num(f[4])?,
            bv: if f[5].trim().is_empty() {
                0.6
            } else {
                num(f[5])?
            },
        });
    }
    Ok(stars)
}

/// Where the catalogue is looked for: next to the working directory (`crates/viewer` or the
/// repository root), next to the executable, then in the crate's source tree.
fn candidates() -> Vec<PathBuf> {
    let mut out = vec![
        PathBuf::from(CATALOGUE),
        Path::new("crates/viewer").join(CATALOGUE),
    ];
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        out.push(dir.join(CATALOGUE));
    }
    out.push(Path::new(env!("CARGO_MANIFEST_DIR")).join(CATALOGUE));
    out
}

/// Loads the shipped catalogue; an empty sky (with a warning) if it cannot be found.
pub fn load() -> Vec<Star> {
    for path in candidates() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            match parse(&text) {
                Ok(stars) => return stars,
                Err(e) => {
                    bevy::log::warn!("star catalogue {}: {e:#}", path.display());
                    return Vec::new();
                }
            }
        }
    }
    bevy::log::warn!("star catalogue {CATALOGUE} not found: no stars");
    Vec::new()
}

/// Linear RGB tint of a star from its B−V index (blue-white to orange-red), normalised so the
/// brightest channel is 1. Black-body temperature from Ballesteros (2012), colour from a
/// fit of the Planck locus.
pub fn color(bv: f64) -> [f32; 3] {
    let bv = bv.clamp(-0.4, 2.0);
    let t = 4_600.0 * (1.0 / (0.92 * bv + 1.7) + 1.0 / (0.92 * bv + 0.62));
    let t = t / 100.0;
    let r = if t <= 66.0 {
        1.0
    } else {
        (1.292_936 * (t - 60.0).powf(-0.133_204_76)).clamp(0.0, 1.0)
    };
    let g = if t <= 66.0 {
        (0.390_081_58 * t.ln() - 0.631_841_4).clamp(0.0, 1.0)
    } else {
        (1.129_890_9 * (t - 60.0).powf(-0.075_514_85)).clamp(0.0, 1.0)
    };
    let b = if t >= 66.0 {
        1.0
    } else if t <= 19.0 {
        0.0
    } else {
        (0.543_206_8 * (t - 10.0).ln() - 1.196_254_1).clamp(0.0, 1.0)
    };
    // sRGB-ish values to linear, then normalised.
    let lin = |c: f64| c.powf(2.2);
    let (r, g, b) = (lin(r), lin(g), lin(b));
    let m = r.max(g).max(b).max(1e-6);
    [(r / m) as f32, (g / m) as f32, (b / m) as f32]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> Vec<Star> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(CATALOGUE);
        parse(&std::fs::read_to_string(path).expect("catalogue")).expect("parse")
    }

    #[test]
    fn shipped_catalogue_is_sane() {
        let stars = shipped();
        assert!(stars.len() >= 8_000, "{} stars", stars.len());
        let faintest = stars.iter().map(|s| s.vmag).fold(f64::MIN, f64::max);
        assert!(
            faintest <= FAINTEST && faintest > FAINTEST - 0.05,
            "{faintest}"
        );
        for s in &stars {
            assert!((0.0..std::f64::consts::TAU).contains(&s.ra));
            assert!((-90f64.to_radians()..=90f64.to_radians()).contains(&s.dec));
            assert!((-1.5..=FAINTEST).contains(&s.vmag));
        }
        // Sirius: α = 6 h 45 min 09 s, δ = −16° 42′ 58″, V = −1.46.
        let sirius = stars
            .iter()
            .min_by(|a, b| a.vmag.total_cmp(&b.vmag))
            .unwrap();
        assert!((sirius.ra.to_degrees() - 101.2871).abs() < 1e-3);
        assert!((sirius.dec.to_degrees() + 16.7161).abs() < 1e-3);
        assert!((sirius.vmag + 1.46).abs() < 1e-9);
    }

    #[test]
    fn colours_go_from_blue_to_red() {
        let blue = color(-0.3);
        let sun = color(0.65);
        let red = color(1.8);
        assert!(blue[2] >= blue[0], "{blue:?}");
        assert!(red[0] > red[2], "{red:?}");
        assert!(sun[0] >= sun[2] && sun[2] > 0.5, "{sun:?}");
    }

    #[test]
    fn parse_rejects_bad_lines() {
        assert!(parse("hr,name,ra_deg,dec_deg,vmag,bv\n1,x,10,20\n").is_err());
        let s = parse("# c\n1,A,90,-45,1.5,\n").unwrap();
        assert_eq!(s[0].bv, 0.6);
    }
}
