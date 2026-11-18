//! Nascio authoritative server (S3, `docs/roadmap-serveur.md`).
//!
//! One process: QUIC connections on the tokio runtime, the simulation on its own thread at a
//! fixed tick ([`sim`]), the journal of the deltas forced to the disk on another thread every
//! `fsync_interval_ms` (group commit), and chunk columns built once and shared by every
//! client ([`columns`]). The server is the only source of generation (invariant 5).

pub mod accounts;
pub mod columns;
pub mod config;
pub mod net;
pub mod replicate;
pub mod security;
pub mod sim;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Context;
use nascio_core::torus;
use nascio_protocol::messages::ServerHello;
use nascio_voxel::delta::write_durable;
use nascio_voxel::{ColumnSet, DeltaConfig, DeltaStore, GenConfig, Terrain};
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

impl RunningServer {
    /// Starts a server for `world`; deltas and the certificate live in `data_dir`
    /// (`deltas/`, `server/`). Call from inside a tokio runtime.
    pub fn start(world: &World, cfg: ServerConfig, data_dir: &Path) -> anyhow::Result<Self> {
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
            eprintln!("deltas: {w}");
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
        // Spawn on the surface of the spawn column.
        let (sx, sz) = (
            torus::wrap_f64(cfg.spawn_xz[0]),
            torus::wrap_f64(cfg.spawn_xz[1]),
        );
        let col = ColumnSet::new(&terrain, (sx / 32.0) as i32, (sz / 32.0) as i32);
        let sy = f64::from(col.top_range().1.max(0) + 2);
        let blocks = terrain.blocks();
        let hello = ServerHello {
            protocol: nascio_protocol::PROTOCOL_VERSION,
            generator_version: GENERATOR_VERSION,
            fingerprint,
            blocks: (0..blocks.len())
                .map(|i| blocks.def(i as u16).name.clone())
                .collect(),
            server_unix_ms: 0,
            player: 0,
            spawn: [sx, sy, sz],
            chunk_radius_m: cfg.chunk_radius_m,
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
        let endpoint = quinn::Endpoint::server(server_cfg, cfg.bind)
            .with_context(|| format!("listening on {}", cfg.bind))?;
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
                            eprintln!("deltas: flush failed: {e:#}");
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
        let shared = Arc::new(net::Shared {
            accounts,
            abuse,
            logins: Default::default(),
            sessions: Default::default(),
            cfg,
            hello,
            cache,
            inputs: Mutex::new(tx),
            next_id: AtomicU32::new(1),
            online: AtomicU32::new(0),
            sent_bytes: AtomicU64::new(0),
            sent_columns: AtomicU64::new(0),
        });
        tokio::spawn(net::accept_loop(endpoint.clone(), shared.clone()));
        Ok(Self {
            addr,
            cert_der,
            shared,
            deltas,
            stats,
            endpoint,
            stop,
            threads,
        })
    }

    /// Closes the connections, stops the threads and makes every edit durable.
    pub fn shutdown(self) -> anyhow::Result<()> {
        self.endpoint.close(0u32.into(), b"shutdown");
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads {
            let _ = t.join();
        }
        self.deltas.flush()?;
        Ok(())
    }
}
