//! Headless bots against a running server: each connects, receives the columns around the
//! spawn and reports when it holds all of them. The seed of the load test (S6).
//!
//! ```text
//! cargo run -p nascio-server --release -- out/m7/world-42 --bind 127.0.0.1:7777
//! cargo run -p nascio-tools --release --bin netbot -- 127.0.0.1:7777 out/m7/world-42/server/cert.der --bots 4
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use nascio_client::{Event, Mirror, connect};
use nascio_protocol::messages::ServerMsg;

#[derive(Parser)]
#[command(name = "netbot", about = "Nascio headless bots")]
struct Cli {
    addr: std::net::SocketAddr,
    /// Server certificate (DER), pinned.
    cert: PathBuf,
    #[arg(long, default_value_t = 1)]
    bots: usize,
    /// Give up after this many seconds.
    #[arg(long, default_value_t = 120.0)]
    timeout: f64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cert = std::fs::read(&cli.cert)?;
    let mut tasks = Vec::new();
    for k in 0..cli.bots {
        let cert = cert.clone();
        let timeout = cli.timeout;
        tasks.push(tokio::spawn(async move {
            let start = Instant::now();
            let mut c = connect(cli.addr, &cert, &format!("bot{k}")).await?;
            let r = c.hello.chunk_radius_m.div_ceil(32) as i32;
            let want = (-r..=r)
                .flat_map(|dz| (-r..=r).map(move |dx| (dx, dz)))
                .filter(|&(dx, dz)| dx * dx + dz * dz <= r * r)
                .count();
            let mut m = Mirror::default();
            let mut bytes = 0usize;
            let mut half = None;
            while m.len() < want && start.elapsed().as_secs_f64() < timeout {
                match c.next(Duration::from_millis(500)).await {
                    Some(Event::Column(col)) => {
                        bytes += col.chunks.len();
                        m.column(col);
                        if half.is_none() && m.len() * 2 >= want {
                            half = Some(start.elapsed());
                        }
                    }
                    Some(Event::Msg(ServerMsg::Blocks {
                        chunk,
                        lsn,
                        changes,
                    })) => m.blocks(chunk, lsn, changes),
                    Some(Event::Closed(r)) => anyhow::bail!("closed: {r}"),
                    _ => {}
                }
            }
            let t = start.elapsed();
            c.close();
            Ok::<_, anyhow::Error>((k, m.len(), want, half.unwrap_or(t), t, bytes))
        }));
    }
    for t in tasks {
        let (k, got, want, half, all, chunks) = t.await??;
        println!(
            "bot{k}: {got}/{want} columns ({chunks} chunks), half in {:.1} s, all in {:.1} s",
            half.as_secs_f64(),
            all.as_secs_f64()
        );
    }
    Ok(())
}
