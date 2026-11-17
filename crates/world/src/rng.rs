//! Deterministic randomness: every generation stage gets its own ChaCha stream derived from
//! `(world seed, stage label)`, so adding a stage never shifts the random numbers of another.

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// 32-byte seed for a stage, `blake3("nascio-stage" ‖ seed ‖ label)`.
pub fn stage_seed(seed: u64, label: &str) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"nascio-stage");
    h.update(&seed.to_le_bytes());
    h.update(label.as_bytes());
    *h.finalize().as_bytes()
}

/// A 64-bit value derived from `(seed, label)`, e.g. to seed a noise function.
pub fn stage_u64(seed: u64, label: &str) -> u64 {
    let b = stage_seed(seed, label);
    u64::from_le_bytes(b[..8].try_into().expect("8 bytes"))
}

/// The RNG of a stage.
pub fn stage_rng(seed: u64, label: &str) -> ChaCha8Rng {
    ChaCha8Rng::from_seed(stage_seed(seed, label))
}

/// SplitMix64 finaliser: a fast, well-mixed integer hash.
#[inline]
pub fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngExt;

    #[test]
    fn same_inputs_same_stream() {
        let a: Vec<u64> = (0..8)
            .map(|_| 0)
            .scan(stage_rng(42, "plates"), |r, _| Some(r.random()))
            .collect();
        let b: Vec<u64> = (0..8)
            .map(|_| 0)
            .scan(stage_rng(42, "plates"), |r, _| Some(r.random()))
            .collect();
        assert_eq!(a, b);
    }

    #[test]
    fn labels_and_seeds_separate_streams() {
        assert_ne!(stage_u64(42, "plates"), stage_u64(42, "crust"));
        assert_ne!(stage_u64(42, "plates"), stage_u64(43, "plates"));
    }
}
