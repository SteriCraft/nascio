//! Server settings (TOML, every field optional).

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// UDP address to listen on.
    pub bind: SocketAddr,
    /// Simulation ticks per second.
    pub tick_hz: u32,
    /// The journal of the deltas is forced to the disk this often (group commit), ms.
    pub fsync_interval_ms: u64,
    /// Radius of the chunk columns sent around each player, metres.
    pub chunk_radius_m: u32,
    /// Columns are dropped by a client beyond the radius plus this margin, metres.
    pub forget_margin_m: u32,
    /// Bytes per second a client may receive (columns for now; entities come in S5).
    pub client_budget_bytes_per_s: u32,
    /// Columns being prepared or sent at once for one client.
    pub columns_in_flight: usize,
    /// Compressed columns kept in memory.
    pub cache_columns: usize,
    /// Threads generating columns (0: all cores but two).
    pub generation_threads: usize,
    /// Players at once.
    pub max_players: usize,
    /// Spawn point (x, z in metres); y is the surface plus two blocks.
    pub spawn_xz: [f64; 2],
    /// Journal size above which the modified regions are written to their files, bytes.
    pub checkpoint_bytes: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:7777".parse().expect("address"),
            tick_hz: 20,
            fsync_interval_ms: 50,
            chunk_radius_m: 512,
            forget_margin_m: 64,
            client_budget_bytes_per_s: 128 << 10,
            columns_in_flight: 16,
            cache_columns: 32_768,
            generation_threads: 0,
            max_players: 1_000,
            spawn_xz: [50_000.0, 50_000.0],
            checkpoint_bytes: 16 << 20,
        }
    }
}

impl ServerConfig {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_and_unknown_keys_fail() {
        let c = ServerConfig::default();
        assert_eq!(
            toml::from_str::<ServerConfig>(&toml::to_string(&c).unwrap()).unwrap(),
            c
        );
        assert_eq!(
            toml::from_str::<ServerConfig>("tick_hz = 10")
                .unwrap()
                .tick_hz,
            10
        );
        assert!(toml::from_str::<ServerConfig>("tick = 10").is_err());
    }
}
