//! Nascio authoritative server (S3, `docs/roadmap-serveur.md`).
//!
//! One process: QUIC connections on the tokio runtime, the simulation on its own thread at a
//! fixed tick ([`sim`]), the journal of the deltas forced to the disk on another thread every
//! `fsync_interval_ms` (group commit), and chunk columns built once and shared by every
//! client ([`columns`]). The server is the only source of generation (invariant 5).

pub mod accounts;
pub mod columns;
pub mod config;
pub mod console;
pub mod horizon;
pub mod net;
pub mod replicate;
pub mod security;
pub mod sim;
pub mod spawn;
pub mod token;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context;
use nascio_protocol::messages::ServerHello;
use nascio_voxel::delta::write_durable;
use nascio_voxel::{DeltaConfig, DeltaStore, GenConfig, Terrain};
use nascio_world::config::GENERATOR_VERSION;
use nascio_world::world_io::World;
use quinn::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

pub use config::ServerConfig;
use sim::{Sim, SimStats};

/// A running server.
pub struct RunningServer {
    pub addr: SocketAddr,
    /// The server certificate (DER), pinned by the clients.
    pub cert_der: Vec<u8>,
    pub shared: Arc<net::Shared>,
    pub deltas: Arc<DeltaStore>,
    pub stats: Arc<Mutex<SimStats>>,
    endpoint: quinn::Endpoint,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    /// Answers the local network's scans.
    discovery: Option<tokio::task::JoinHandle<()>>,
}

/// The server's certificate and key in `dir` (`cert.der`, `key.der`), made on first use
/// (self-signed for `localhost`).
pub fn certificate(dir: &Path) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let (cert, key) = (dir.join("cert.der"), dir.join("key.der"));
    if cert.exists() && key.exists() {
        return Ok((std::fs::read(&cert)?, std::fs::read(&key)?));
    }
    std::fs::create_dir_all(dir)?;
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])?;
    let (c, k) = (ck.cert.der().to_vec(), ck.signing_key.serialize_der());
    write_durable(&key, &k)?;
    write_durable(&cert, &c)?;
    Ok((c, k))
}

/// The server's UDP socket, with larger buffers: a rush of connections otherwise overflows
/// the system's default receive buffer, and the lost handshake packets wait for QUIC's
/// retransmission timers.
fn udp_socket(bind: SocketAddr, buffer: usize) -> anyhow::Result<std::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::for_address(bind), Type::DGRAM, Some(Protocol::UDP))?;
    if buffer > 0 {
        // Best effort: the system may cap them (Linux: net.core.rmem_max / wmem_max).
        let _ = s.set_recv_buffer_size(buffer);
        let _ = s.set_send_buffer_size(buffer);
        let got = s.recv_buffer_size().unwrap_or(0);
        if got < buffer {
            crate::say_err!(
                "UDP receive buffer {} KiB instead of {} KiB (Linux: raise net.core.rmem_max)",
                got >> 10,
                buffer >> 10
            );
        }
    }
    s.bind(&bind.into())?;
    Ok(s.into())
}

impl RunningServer {
    /// Starts a server for `world`; deltas and the certificate live in `data_dir`
    /// (`deltas/`, `server/`). Call from inside a tokio runtime.
    pub fn start(world: &World, cfg: ServerConfig, data_dir: &Path) -> anyhow::Result<Self> {
        cfg.validate()?;
        let gen_cfg = GenConfig::standard();
        let fingerprint = gen_cfg.fingerprint(world);
        let terrain = Arc::new(Terrain::from_world(world, gen_cfg.voxel.clone())?);
        let (deltas, warnings) = DeltaStore::open(
            &data_dir.join("deltas"),
            GENERATOR_VERSION,
            &fingerprint,
            terrain.blocks(),
            DeltaConfig {
                palette_above: gen_cfg.voxel.delta_palette_above,
                checkpoint_bytes: cfg.checkpoint_bytes,
                ..DeltaConfig::default()
            },
        )?;
        for w in warnings {
            crate::say_err!("deltas: {w}");
        }
        let deltas = Arc::new(deltas);
        let threads_n = match cfg.generation_threads {
            0 => std::thread::available_parallelism()
                .map_or(2, |n| n.get())
                .saturating_sub(2)
                .max(1),
            n => n,
        };
        let cache = Arc::new(columns::ColumnCache::new(
            terrain.clone(),
            deltas.clone(),
            cfg.cache_columns,
            threads_n,
        ));
        let tiles = Arc::new(horizon::TileCache::new(
            terrain.clone(),
            cfg.cache_tiles,
            threads_n,
        ));
        let started = std::time::Instant::now();
        let sky: Arc<[u8]> = horizon::world_sky(world, &terrain)?.into();
        crate::say!(
            "sky and far ground ready: {} KB in {:.1} s",
            sky.len() >> 10,
            started.elapsed().as_secs_f64()
        );
        let spawns = spawn::Spawns::new(
            terrain.clone(),
            world,
            &fingerprint,
            &cfg,
            &data_dir.join("server"),
        )?;
        // Each login replaces it with the player's point.
        let first = spawns.fixed.or(spawns.sites.first().copied());
        let spawn_pos = first.map_or([0.0; 3], |p| spawn::standing(&terrain, p));
        let blocks = terrain.blocks();
        let hello = ServerHello {
            protocol: nascio_protocol::PROTOCOL_VERSION,
            generator_version: GENERATOR_VERSION,
            fingerprint,
            blocks: blocks.registry().blocks.clone(),
            server_unix_ms: 0,
            player: 0,
            spawn: spawn_pos,
            chunk_radius_m: cfg.chunk_radius_m,
            physics: cfg.player.clone(),
            mode: nascio_physics::Mode::Walk,
        };

        let (cert_der, key_der) = certificate(&data_dir.join("server"))?;
        let mut server_cfg = quinn::ServerConfig::with_single_cert(
            vec![CertificateDer::from(cert_der.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
        )?;
        let mut transport = quinn::TransportConfig::default();
        transport.max_idle_timeout(Some(Duration::from_secs(30).try_into()?));
        // Entity datagrams: at most four ticks queued per client (quinn's default, 1 MB,
        // would hold seconds of stale states when a client falls behind).
        let per_tick = (cfg.client_budget_bytes_per_s / cfg.tick_hz.max(1)) as usize;
        transport.datagram_send_buffer_size(4 * per_tick.max(cfg.max_datagram_bytes));
        server_cfg.transport_config(Arc::new(transport));
        let socket = udp_socket(cfg.bind, cfg.udp_buffer_bytes)
            .with_context(|| format!("listening on {}", cfg.bind))?;
        let endpoint = quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            Some(server_cfg),
            socket,
            quinn::default_runtime().context("no async runtime")?,
        )?;
        let addr = endpoint.local_addr()?;

        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(SimStats::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        let sim = Sim {
            deltas: deltas.clone(),
            cache: cache.clone(),
            blocks: blocks.len(),
            tick: Duration::from_secs_f64(1.0 / f64::from(cfg.tick_hz.max(1))),
            stats: stats.clone(),
            repl: replicate::ReplConfig {
                radius_m: cfg.entity_radius_m,
                fast: cfg.entity_fast,
                max_gap: (cfg.tick_hz / cfg.entity_min_hz.max(1)).max(1),
                bytes_per_tick: (cfg.client_budget_bytes_per_s / cfg.tick_hz.max(1)) as usize,
                max_datagram: cfg.max_datagram_bytes,
            },
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(cfg.replication_threads.max(1))
                .thread_name(|i| format!("replication-{i}"))
                .build()?,
        };
        let mut threads = Vec::new();
        let s = stop.clone();
        threads.push(
            std::thread::Builder::new()
                .name("sim".into())
                .spawn(move || sim.run(rx, s))?,
        );
        // Group commit of the journal.
        let (d, s) = (deltas.clone(), stop.clone());
        let every = Duration::from_millis(cfg.fsync_interval_ms.max(1));
        threads.push(
            std::thread::Builder::new()
                .name("fsync".into())
                .spawn(move || {
                    while !s.load(Ordering::Relaxed) {
                        std::thread::sleep(every);
                        if let Err(e) = d.flush() {
                            crate::say_err!("deltas: flush failed: {e:#}");
                        }
                    }
                })?,
        );
        let server_dir = data_dir.join("server");
        let accounts = accounts::Accounts::open_with(
            &server_dir,
            argon2::Params::new(cfg.account_hash_kib, cfg.account_hash_passes, 1, Some(32))
                .map_err(|e| anyhow::anyhow!("argon2 parameters: {e}"))?,
        )?;
        let abuse = security::AbuseLog::open(&server_dir.join("abuse.log"))?;
        let hash_permits = match cfg.account_hash_concurrency {
            0 => (std::thread::available_parallelism().map_or(2, |n| n.get()) / 2).max(1),
            n => n,
        };
        let tokens = if cfg.site_public_key.is_empty() {
            None
        } else {
            let key = token::b64_decode(cfg.site_public_key.trim())
                .context("site_public_key is not base64url")?;
            Some(
                token::Verifier::new(&key, &cfg.server_id, net::unix_ms())
                    .context("site_public_key")?,
            )
        };
        let shared = Arc::new(net::Shared {
            seed: world.config.seed,
            tokens,
            hashing: tokio::sync::Semaphore::new(hash_permits),
            accounts,
            abuse,
            logins: Default::default(),
            sessions: Default::default(),
            spawns,
            cfg,
            hello,
            cache,
            tiles,
            sky,
            inputs: Mutex::new(tx),
            next_id: AtomicU32::new(1),
            online: AtomicU32::new(0),
            sent_bytes: AtomicU64::new(0),
            sent_columns: AtomicU64::new(0),
            sent_skies: AtomicU64::new(0),
            sent_tiles: AtomicU64::new(0),
        });
        tokio::spawn(net::accept_loop(endpoint.clone(), shared.clone()));
        let discovery = match shared.cfg.discovery_port {
            0 => None,
            port => {
                let at = SocketAddr::new(addr.ip(), port);
                match std::net::UdpSocket::bind(at)
                    .and_then(|s| s.set_nonblocking(true).map(|()| s))
                    .and_then(tokio::net::UdpSocket::from_std)
                {
                    Ok(socket) => Some(tokio::spawn(discovery(
                        socket,
                        shared.clone(),
                        addr.port(),
                        cert_der.clone(),
                    ))),
                    Err(e) => {
                        crate::say_err!("local network scans not answered ({at}: {e})");
                        None
                    }
                }
            }
        };
        Ok(Self {
            addr,
            cert_der,
            shared,
            deltas,
            stats,
            endpoint,
            stop,
            threads,
            discovery,
        })
    }

    /// Closes the connections, stops the threads and makes every edit durable.
    /// Tells every client the server stops and gives the closing frames time to leave (at most
    /// 2 s), so that the clients see a closed connection at once, not a timeout.
    pub async fn close_connections(&self) {
        self.endpoint.close(0u32.into(), b"server stopped");
        let _ = tokio::time::timeout(Duration::from_secs(2), self.endpoint.wait_idle()).await;
    }

    pub fn shutdown(self) -> anyhow::Result<()> {
        if let Some(d) = &self.discovery {
            d.abort();
        }
        self.endpoint.close(0u32.into(), b"shutdown");
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads {
            let _ = t.join();
        }
        self.deltas.flush()?;
        Ok(())
    }
}

/// Answers the probes of the local network with the game port and the certificate. Only
/// full-size probes are answered: the answer is never larger (no amplification).
async fn discovery(
    socket: tokio::net::UdpSocket,
    shared: Arc<net::Shared>,
    port: u16,
    cert_der: Vec<u8>,
) {
    use nascio_protocol::discovery::{PROBE_BYTES, ServerInfo, answer, is_probe};
    let mut buf = vec![0u8; PROBE_BYTES + 1];
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buf).await else {
            // Windows reports an ICMP "port unreachable" of an earlier answer here.
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        if !is_probe(&buf[..n]) {
            continue;
        }
        let info = ServerInfo {
            protocol: nascio_protocol::PROTOCOL_VERSION,
            port,
            server_id: shared.cfg.server_id.clone(),
            players: shared.online.load(Ordering::Relaxed),
            max_players: u32::try_from(shared.cfg.max_players).unwrap_or(u32::MAX),
            cert_der: cert_der.clone(),
        };
        if let Some(a) = answer(&info) {
            let _ = socket.send_to(&a, from).await;
        }
    }
}
