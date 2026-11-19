//! Headless bots against a running server: each connects, receives the columns around the
//! spawn and reports when it holds all of them. The seed of the load test (S6).
//!
//! With `--walk`, the bots then walk in circles from their spawn for `--hold` seconds
//! (commands sent at 20 Hz, moved by the server), to be seen in the viewer (`--connect`).
//!
//! ```text
//! cargo run -p nascio-server --release -- out/m7/world-42 --bind 127.0.0.1:7777 --open-registration
//! cargo run -p nascio-tools --release --bin netbot -- 127.0.0.1:7777 out/m7/world-42/server/cert.der --bots 4
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use nascio_client::{Event, Mirror, connect};
use nascio_physics::{Input, keys};
use nascio_protocol::entity::quantize_pitch;
use nascio_protocol::messages::ServerMsg;
use nascio_protocol::player::Commands;

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
    /// Walk in circles once the columns are in.
    #[arg(long)]
    walk: bool,
    /// Seconds of walking.
    #[arg(long, default_value_t = 60.0)]
    hold: f64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cert = std::fs::read(&cli.cert).map_err(|e| {
        anyhow::anyhow!(
            "server certificate {}: {e} (copy the server's <data>/server/cert.der here)",
            cli.cert.display()
        )
    })?;
    let mut tasks = Vec::new();
    for k in 0..cli.bots {
        let cert = cert.clone();
        let timeout = cli.timeout;
        let (walk, hold) = (cli.walk, cli.hold);
        tasks.push(tokio::spawn(async move {
            let start = Instant::now();
            let mut c = connect(cli.addr, &cert, &format!("bot{k}"), "netbot-dev-secret").await?;
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
                        m.column(*col);
                        if half.is_none() && m.len() * 2 >= want {
                            half = Some(start.elapsed());
                        }
                    }
                    Some(Event::Msg(ServerMsg::Blocks {
                        chunk,
                        lsn,
                        changes,
                    })) => {
                        m.blocks(chunk, lsn, changes);
                    }
                    Some(Event::Closed(r)) => anyhow::bail!("closed: {r}"),
                    _ => {}
                }
            }
            let t = start.elapsed();
            if walk {
                // One turn in 10 s, alternate directions (a circle of about 7 m at walking
                // speed, from where the server has the bot).
                let sense = if k % 2 == 0 { 1.0 } else { -1.0 };
                // Walks along the tangent, three steps every 50 ms; the server moves it
                // (chantier 5.3).
                let begin = Instant::now();
                let mut every = tokio::time::interval(Duration::from_millis(50));
                let mut last = 2u32;
                while begin.elapsed().as_secs_f64() < hold {
                    every.tick().await;
                    let a = sense * begin.elapsed().as_secs_f64() * std::f64::consts::TAU / 10.0;
                    // Direction of motion on the circle, as (dx, dz).
                    let dir = [-sense * a.sin(), sense * a.cos()];
                    let step = Input {
                        keys: keys::FORWARD,
                        yaw: Input::quantize_yaw(dir[0].atan2(-dir[1])),
                        fly_speed: 0,
                    };
                    c.send_commands(&Commands {
                        last,
                        pitch: quantize_pitch(0.0),
                        inputs: vec![step; 3],
                    });
                    last += 3;
                    // Steps the server took without commands are numbered after ours: the
                    // next commands follow its acknowledgement.
                    while let Ok(ev) = c.events.try_recv() {
                        if let nascio_client::Event::Player(p) = ev
                            && let Some(a) = p.ack
                        {
                            last = last.max(a + 3);
                        }
                    }
                }
            }
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
