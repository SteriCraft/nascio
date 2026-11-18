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
    /// One spawn point for every player (x, z in metres; tests, events), standing on the
    /// surface. Absent: each account gets its own point near a spawn site.
    pub spawn_xz: Option<[f64; 2]>,
    /// Spawn sites of the world, drawn from its seed at the first start
    /// (`<data>/server/spawn_sites.toml`).
    pub spawn_sites: usize,
    /// A new account's point lies within this distance of a site drawn at random (m).
    pub spawn_radius_m: f64,
    /// Sites and points stand lower than this (m); they are on dry land, outside the polar
    /// band.
    pub spawn_max_elevation_m: f64,
    /// An unknown name creates its account at login (otherwise accounts are made with the
    /// console's `adduser`). For development and tests; off by default.
    pub open_registration: bool,
    /// Who may log in with a secret: `admin` (administrators only), `all` or `none`. Players
    /// log in with a token from the site.
    pub secret_login: SecretLogin,
    /// The site's Ed25519 public key (base64url, from `site keygen`); empty: no tokens.
    pub site_public_key: String,
    /// This server's name in the tokens (their audience). `dev` is refused unless the server
    /// listens on a local address only: a public server needs its own name, or a token made
    /// for another `dev` server would open it.
    pub server_id: String,
    /// Argon2id cost of new accounts: memory (KiB) and passes.
    pub account_hash_kib: u32,
    pub account_hash_passes: u32,
    /// Secret hashes computed at once (0: half the cores). Logins beyond wait their turn
    /// instead of taking every core during a rush of connections.
    pub account_hash_concurrency: usize,
    /// Receive and send buffers asked for the UDP socket, bytes (0: the system default).
    /// On Linux the kernel caps them at `net.core.rmem_max` / `wmem_max`.
    pub udp_buffer_bytes: usize,
    /// Failed logins allowed per address and minute.
    pub login_failures_per_min: usize,
    /// Control messages a client may send per second, and in a burst.
    pub control_msgs_per_s: f64,
    pub control_burst: f64,
    /// Block edits a client may ask per second, and in a burst.
    pub edits_per_s: f64,
    pub edits_burst: f64,
    /// Chat lines a client may send per second, and in a burst (more are dropped).
    pub chat_per_s: f64,
    pub chat_burst: f64,
    /// UDP port answering the local network's scans (`nascio_protocol::discovery`), on the
    /// address of `bind` (0: no answer).
    pub discovery_port: u16,
    /// Refused messages (over a limit) after which a client is kicked.
    pub kick_after_violations: u32,
    /// Positions and acks over the limit are dropped without counting as violations (a
    /// backlog after a stall of the link), but a client still over the limit after this
    /// many seconds, with no pause of `Overrun::GAP`, is kicked (0: never).
    pub overrun_kick_after_s: f64,
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
            bind: "127.0.0.1:7777".parse().expect("address"),
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
            spawn_xz: None,
            spawn_sites: 10,
            spawn_radius_m: 300.0,
            spawn_max_elevation_m: 1_000.0,
            checkpoint_bytes: 16 << 20,
            open_registration: false,
            secret_login: SecretLogin::Admin,
            site_public_key: String::new(),
            server_id: DEV_SERVER_ID.to_owned(),
            account_hash_kib: 19 * 1024,
            account_hash_passes: 2,
            account_hash_concurrency: 0,
            udp_buffer_bytes: 4 << 20,
            login_failures_per_min: 5,
            control_msgs_per_s: 200.0,
            control_burst: 400.0,
            edits_per_s: 20.0,
            edits_burst: 40.0,
            chat_per_s: 1.0,
            chat_burst: 5.0,
            discovery_port: nascio_protocol::discovery::DISCOVERY_PORT,
            kick_after_violations: 500,
            overrun_kick_after_s: 10.0,
            max_speed_m_s: 120.0,
            edit_reach_m: 160.0,
        }
    }
}

/// Who may log in with a secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretLogin {
    /// Administrators only (`admin <name> on`, or `adduser … admin`).
    Admin,
    /// Every account (development; `open_registration` then creates accounts).
    All,
    /// Nobody: tokens only.
    None,
}

/// The default `server_id`, for servers on this machine only.
pub const DEV_SERVER_ID: &str = "dev";

impl ServerConfig {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
    }

    /// Settings the server refuses to start with.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.server_id.trim().is_empty(), "server_id is empty");
        anyhow::ensure!(
            self.server_id != DEV_SERVER_ID || self.bind.ip().is_loopback(),
            "server_id \"{DEV_SERVER_ID}\" is for local servers: give this server its own server_id (or --server-id) to listen on {}",
            self.bind
        );
        anyhow::ensure!(
            (1..=1_000).contains(&self.spawn_sites),
            "spawn_sites is between 1 and 1000"
        );
        anyhow::ensure!(
            self.spawn_radius_m >= 0.0 && self.spawn_radius_m <= 10_000.0,
            "spawn_radius_m is between 0 and 10000"
        );
        Ok(())
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
        assert_eq!(
            toml::from_str::<ServerConfig>("secret_login = \"none\"")
                .unwrap()
                .secret_login,
            SecretLogin::None
        );
    }

    #[test]
    fn a_public_server_needs_its_own_id() {
        let mut c = ServerConfig::default();
        assert!(c.validate().is_ok(), "local by default");
        for bind in ["0.0.0.0:7777", "192.168.1.128:7777", "[::]:7777"] {
            c.bind = bind.parse().unwrap();
            assert!(c.validate().is_err(), "{bind}");
        }
        c.server_id = "eu-1".into();
        assert!(c.validate().is_ok());
        c.server_id = " ".into();
        assert!(c.validate().is_err());
        c.server_id = DEV_SERVER_ID.into();
        c.bind = "[::1]:7777".parse().unwrap();
        assert!(c.validate().is_ok());
    }
}
