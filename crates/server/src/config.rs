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
    /// Bytes per second a client may receive: entities first, then edits, then columns.
    pub client_budget_bytes_per_s: u32,
    /// Entities within this distance of a player are replicated to them, metres.
    pub entity_radius_m: f64,
    /// Entities sent each tick by priority (the closest, the longest unsent).
    pub entity_fast: usize,
    /// Every entity in range is sent at least this often, Hz.
    pub entity_min_hz: u32,
    /// Size of an entity datagram at most, bytes (lowered to what the path allows).
    pub max_datagram_bytes: usize,
    /// Threads replicating the entities at the end of a tick (one client per task).
    pub replication_threads: usize,
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
    /// An unknown name creates its account at login (otherwise accounts are made with the
    /// console's `adduser`).
    pub open_registration: bool,
    /// Argon2id cost of new accounts: memory (KiB) and passes.
    pub account_hash_kib: u32,
    pub account_hash_passes: u32,
    /// Failed logins allowed per address and minute.
    pub login_failures_per_min: usize,
    /// Control messages a client may send per second, and in a burst.
    pub control_msgs_per_s: f64,
    pub control_burst: f64,
    /// Block edits a client may ask per second, and in a burst.
    pub edits_per_s: f64,
    pub edits_burst: f64,
    /// Refused messages (over a limit) after which a client is kicked.
    pub kick_after_violations: u32,
    /// Fastest move accepted, m/s (0: not checked). A gameplay value, to be set by the
    /// author; the default lets the viewer fly.
    pub max_speed_m_s: f64,
    /// Farthest block a player may edit from their position, m (0: not checked). A
    /// gameplay value, to be set by the author.
    pub edit_reach_m: f64,
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
            entity_radius_m: 256.0,
            entity_fast: 64,
            entity_min_hz: 5,
            max_datagram_bytes: 1_200,
            replication_threads: 4,
            columns_in_flight: 16,
            cache_columns: 32_768,
            generation_threads: 0,
            max_players: 1_000,
            spawn_xz: [50_000.0, 50_000.0],
            checkpoint_bytes: 16 << 20,
            open_registration: true,
            account_hash_kib: 19 * 1024,
            account_hash_passes: 2,
            login_failures_per_min: 5,
            control_msgs_per_s: 200.0,
            control_burst: 400.0,
            edits_per_s: 20.0,
            edits_burst: 40.0,
            kick_after_violations: 500,
            max_speed_m_s: 120.0,
            edit_reach_m: 160.0,
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
