//! `nascio-server <world dir>`: serves a generated world. Deltas live in
//! `<data>/deltas/`, the certificate in `<data>/server/` (clients pin `cert.der`); `<data>`
//! is the world folder unless `--data` says otherwise.
//!
//! ```text
//! cargo run -p nascio-server --release -- out/m7/world-42 --bind 127.0.0.1:7777
//! ```

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use nascio_server::{RunningServer, ServerConfig};
use nascio_world::world_io::World;

#[derive(Parser)]
#[command(name = "nascio-server", about = "Nascio authoritative server")]
struct Cli {
    world: PathBuf,
    /// TOML settings (`ServerConfig`).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Folder of the deltas and the certificate (default: the world folder).
    #[arg(long)]
    data: Option<PathBuf>,
    /// Overrides `bind`.
    #[arg(long)]
    bind: Option<std::net::SocketAddr>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut cfg = match &cli.config {
        Some(p) => ServerConfig::load(p)?,
        None => ServerConfig::default(),
    };
    if let Some(b) = cli.bind {
        cfg.bind = b;
    }
    let world = World::load(&cli.world)?;
    let data = cli.data.clone().unwrap_or_else(|| cli.world.clone());
    let server = RunningServer::start(&world, cfg, &data)?;
    println!(
        "listening on {} — certificate {}",
        server.addr,
        data.join("server").join("cert.der").display()
    );
    let mut every = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = every.tick() => {
                let s = server.stats.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let sh = &server.shared;
                println!(
                    "players {} · tick p50 {:.2} ms p99 {:.2} ms · edits {} (refused {}) · columns built {} sent {} ({:.1} MB) · entities {} datagrams ({:.1} MB)",
                    s.players,
                    s.tick_ms(0.5),
                    s.tick_ms(0.99),
                    s.edits,
                    s.refused,
                    sh.cache.built.load(std::sync::atomic::Ordering::Relaxed),
                    sh.sent_columns.load(std::sync::atomic::Ordering::Relaxed),
                    sh.sent_bytes.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6,
                    s.entity_datagrams,
                    s.entity_bytes as f64 / 1e6,
                );
            }
        }
    }
    println!("stopping");
    server.shutdown()
}
