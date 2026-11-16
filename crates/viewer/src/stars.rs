//! Bright-star catalogue (spec §2.8: real stars, equatorial coordinates J2000).
//!
//! The ~90 brightest stars of the night sky (down to magnitude ≈ 3): right ascension in hours,
//! declination in degrees, visual magnitude. Positions and magnitudes are public astronomical
//! facts (Yale Bright Star Catalogue values, rounded).

/// `(right ascension h, declination °, magnitude)`.
pub const STARS: [(f64, f64, f64); 92] = [
    (6.752, -16.716, -1.46),  // Sirius
    (6.399, -52.696, -0.74),  // Canopus
    (14.660, -60.834, -0.27), // Rigil Kentaurus
    (14.261, 19.182, -0.05),  // Arcturus
    (18.616, 38.784, 0.03),   // Vega
    (5.278, 45.998, 0.08),    // Capella
    (5.242, -8.202, 0.13),    // Rigel
    (7.655, 5.225, 0.34),     // Procyon
    (1.629, -57.237, 0.46),   // Achernar
    (5.919, 7.407, 0.50),     // Betelgeuse
    (14.064, -60.373, 0.61),  // Hadar
    (19.846, 8.868, 0.76),    // Altair
    (12.443, -63.099, 0.76),  // Acrux
    (4.599, 16.509, 0.86),    // Aldebaran
    (16.490, -26.432, 0.96),  // Antares
    (13.420, -11.161, 0.97),  // Spica
    (7.755, 28.026, 1.14),    // Pollux
    (22.961, -29.622, 1.16),  // Fomalhaut
    (20.690, 45.280, 1.25),   // Deneb
    (12.795, -59.689, 1.25),  // Mimosa
    (10.140, 11.967, 1.35),   // Regulus
    (6.977, -28.972, 1.50),   // Adhara
    (7.577, 31.888, 1.58),    // Castor
    (17.560, -37.104, 1.62),  // Shaula
    (12.519, -57.113, 1.63),  // Gacrux
    (5.419, 6.350, 1.64),     // Bellatrix
    (5.438, 28.608, 1.65),    // Elnath
    (9.220, -69.717, 1.68),   // Miaplacidus
    (5.604, -1.202, 1.69),    // Alnilam
    (22.137, -46.961, 1.74),  // Alnair
    (12.900, 55.960, 1.76),   // Alioth
    (5.679, -1.943, 1.77),    // Alnitak
    (11.062, 61.751, 1.79),   // Dubhe
    (3.405, 49.861, 1.79),    // Mirfak
    (7.140, -26.393, 1.83),   // Wezen
    (18.403, -34.385, 1.85),  // Kaus Australis
    (8.375, -59.510, 1.86),   // Avior
    (13.792, 49.313, 1.86),   // Alkaid
    (17.622, -42.998, 1.87),  // Sargas
    (5.992, 44.947, 1.90),    // Menkalinan
    (16.811, -69.028, 1.91),  // Atria
    (6.629, 16.399, 1.92),    // Alhena
    (20.427, -56.735, 1.94),  // Peacock
    (2.530, 89.264, 1.98),    // Polaris
    (6.378, -17.956, 1.98),   // Mirzam
    (9.460, -8.659, 1.98),    // Alphard
    (10.333, 19.842, 2.01),   // Algieba
    (2.120, 23.462, 2.00),    // Hamal
    (0.727, -17.987, 2.02),   // Diphda
    (18.921, -26.297, 2.05),  // Nunki
    (14.111, -36.370, 2.06),  // Menkent
    (0.140, 29.091, 2.06),    // Alpheratz
    (1.162, 35.621, 2.05),    // Mirach
    (5.796, -9.670, 2.07),    // Saiph
    (14.845, 74.156, 2.08),   // Kochab
    (17.582, 12.560, 2.08),   // Rasalhague
    (22.711, -46.885, 2.07),  // Tiaki
    (3.136, 40.957, 2.09),    // Algol
    (2.065, 42.330, 2.10),    // Almach
    (11.818, 14.572, 2.13),   // Denebola
    (9.285, -59.275, 2.21),   // Aspidiske
    (9.133, -43.433, 2.21),   // Suhail
    (15.578, 26.715, 2.22),   // Alphecca
    (13.399, 54.925, 2.23),   // Mizar
    (20.370, 40.257, 2.23),   // Sadr
    (5.533, -0.299, 2.23),    // Mintaka
    (0.675, 56.537, 2.24),    // Schedar
    (17.943, 51.489, 2.24),   // Eltanin
    (8.060, -40.003, 2.25),   // Naos
    (0.153, 59.150, 2.27),    // Caph
    (16.006, -22.622, 2.29),  // Dschubba
    (16.836, -34.293, 2.29),  // Larawag
    (11.031, 56.382, 2.37),   // Merak
    (14.750, 27.074, 2.37),   // Izar
    (21.736, 9.875, 2.39),    // Enif
    (0.438, -42.306, 2.40),   // Ankaa
    (23.063, 28.083, 2.42),   // Scheat
    (17.173, -15.725, 2.43),  // Sabik
    (11.897, 53.695, 2.44),   // Phecda
    (7.402, -29.303, 2.45),   // Aludra
    (21.310, 62.585, 2.45),   // Alderamin
    (23.079, 15.205, 2.48),   // Markab
    (3.038, 4.090, 2.54),     // Menkar
    (12.263, -17.542, 2.59),  // Gienah
    (15.283, -9.383, 2.61),   // Zubeneschamali
    (15.738, 6.426, 2.63),    // Unukalhai
    (1.430, 60.235, 2.68),    // Ruchbah
    (19.771, 10.613, 2.72),   // Tarazed
    (14.848, -16.042, 2.75),  // Zubenelgenubi
    (16.504, 21.490, 2.78),   // Kornephoros
    (13.036, 10.959, 2.83),   // Vindemiatrix
    (3.791, 24.105, 2.87),    // Alcyone
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_sane() {
        for &(ra, dec, mag) in &STARS {
            assert!((0.0..24.0).contains(&ra));
            assert!((-90.0..=90.0).contains(&dec));
            assert!((-1.5..3.0).contains(&mag));
        }
    }
}
