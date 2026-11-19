//! Load test (S6): many bots in one process, gathered within a radius around a point (a
//! battle), walking at 20 Hz, acknowledging the entity datagrams, and some of them editing
//! blocks. Reports what each bot receives per second (columns, entities) and how many
//! entities it sees; the server prints its tick times.
//!
//! ```text
//! cargo run -p nascio-server --release -- out/m7/world-42 --data out/s3-data --bind 127.0.0.1:7777 --open-registration
//! cargo run -p nascio-tools --release --bin swarm -- 127.0.0.1:7777 out/s3-data/server/cert.der --bots 500
//! ```
//!
//! With `--site-key` (the key of `site keygen`, whose public key the server holds), each bot
//! logs in with its own token, as players will:
//!
//! ```text
//! cargo run -p nascio-tools --release --bin swarm -- 127.0.0.1:7777 out/s3-data/server/cert.der --site-key out/site.key
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use nascio_client::{Auth, Entities, Event, connect_with};
use nascio_core::torus;
use nascio_physics::{Input, keys};
use nascio_protocol::entity::quantize_pitch;
use nascio_protocol::messages::{ClientMsg, ServerMsg};
use nascio_protocol::player::Commands;
use nascio_server::net::unix_ms;
use nascio_server::token::{MAX_TTL_MS, SiteKey};

#[derive(Parser)]
#[command(name = "swarm", about = "Nascio load test: many bots in a small area")]
struct Cli {
    addr: std::net::SocketAddr,
    /// Server certificate (DER), pinned.
    cert: PathBuf,
    #[arg(long, default_value_t = 500)]
    bots: usize,
    /// Bots stay within this distance of the centre, metres.
    #[arg(long, default_value_t = 200.0)]
    radius: f64,
    /// Centre x,y,z (default: the server's spawn).
    #[arg(long, value_delimiter = ',')]
    centre: Option<Vec<f64>>,
    /// Walking speed, m/s.
    #[arg(long, default_value_t = 4.0)]
    speed: f64,
    /// New connections per second.
    #[arg(long, default_value_t = 50.0)]
    ramp: f64,
    /// Seconds of measurement once every bot is in (after `settle`).
    #[arg(long, default_value_t = 30.0)]
    duration: f64,
    /// Seconds between the last connection and the measurement.
    #[arg(long, default_value_t = 20.0)]
    settle: f64,
    /// Block edits per second, all bots together.
    #[arg(long, default_value_t = 20.0)]
    edits: f64,
    /// Seed of the bots' random walks.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// The site's private key: the bots log in with tokens instead of secrets.
    #[arg(long)]
    site_key: Option<PathBuf>,
    /// The server's `server_id`, for the tokens.
    #[arg(long, default_value = "dev")]
    server_id: String,
}

/// Deterministic pseudo-random numbers in [0, 1).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Counters of one bot (shared with the reporter).
#[derive(Default)]
struct Counters {
    column_bytes: AtomicU64,
    columns: AtomicU64,
    entity_bytes: AtomicU64,
    datagrams: AtomicU64,
    seen: AtomicUsize,
    edits: AtomicU64,
    refused: AtomicU64,
    errors: AtomicU64,
    /// Authoritative states of the bot received.
    states: AtomicU64,
    /// Command datagrams sent (20 a second when the bot keeps up).
    commands: AtomicU64,
    /// Longest gap between two command datagrams, ms.
    command_gap_ms: AtomicU64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Arc::new(Cli::parse());
    anyhow::ensure!(
        cli.centre.as_ref().is_none_or(|c| c.len() == 3),
        "--centre takes x,y,z"
    );
    let cert = Arc::new(read_cert(&cli.cert)?);
    let site = Arc::new(cli.site_key.as_deref().map(SiteKey::load).transpose()?);
    let counters: Arc<Vec<Counters>> =
        Arc::new((0..cli.bots).map(|_| Counters::default()).collect());
    let start = Instant::now();
    let ramp_s = cli.bots as f64 / cli.ramp;
    let end = Duration::from_secs_f64(ramp_s + cli.settle + cli.duration);
    let mut tasks = Vec::new();
    for k in 0..cli.bots {
        let (cli, cert, counters, site) =
            (cli.clone(), cert.clone(), counters.clone(), site.clone());
        tasks.push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f64(k as f64 / cli.ramp)).await;
            if let Err(e) = bot(
                k,
                &cli,
                &cert,
                site.as_ref().as_ref(),
                &counters[k],
                start + end,
            )
            .await
            {
                eprintln!("bot{k}: {e:#}");
                counters[k].errors.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }
    // Snapshot of the counters when the measurement starts and ends.
    let total = |c: &Counters| {
        [
            c.column_bytes.load(Ordering::Relaxed),
            c.entity_bytes.load(Ordering::Relaxed),
            c.datagrams.load(Ordering::Relaxed),
            c.columns.load(Ordering::Relaxed),
        ]
    };
    let mut every = tokio::time::interval(Duration::from_secs(5));
    every.tick().await;
    let measure_at = Duration::from_secs_f64(ramp_s + cli.settle);
    let mut before: Option<Vec<[u64; 4]>> = None;
    while start.elapsed() < end {
        every.tick().await;
        let t = start.elapsed();
        if before.is_none() && t >= measure_at {
            before = Some(counters.iter().map(total).collect());
        }
        let seen: Vec<usize> = counters
            .iter()
            .map(|c| c.seen.load(Ordering::Relaxed))
            .collect();
        let sum: u64 = counters.iter().map(|c| total(c)[0] + total(c)[1]).sum();
        println!(
            "t {:5.1} s · bots {} · seen mean {:.0} · received {:.1} MB",
            t.as_secs_f64(),
            seen.iter().filter(|&&s| s > 0).count(),
            seen.iter().sum::<usize>() as f64 / seen.len().max(1) as f64,
            sum as f64 / 1e6
        );
    }
    let after: Vec<[u64; 4]> = counters.iter().map(total).collect();
    let before = before.unwrap_or_else(|| vec![[0; 4]; counters.len()]);
    let secs = cli.duration;
    let mut per: Vec<(f64, f64, f64)> = before
        .iter()
        .zip(&after)
        .map(|(b, a)| {
            (
                (a[0] - b[0]) as f64 / secs,
                (a[1] - b[1]) as f64 / secs,
                (a[2] - b[2]) as f64 / secs,
            )
        })
        .collect();
    per.sort_by(|a, b| (a.0 + a.1).total_cmp(&(b.0 + b.1)));
    let sum = |f: &dyn Fn(&(f64, f64, f64)) -> f64| per.iter().map(f).sum::<f64>();
    let n = per.len() as f64;
    println!(
        "measured over {secs:.0} s: per bot, columns {:.0} B/s, entities {:.0} B/s ({:.1} datagrams/s)",
        sum(&|p| p.0) / n,
        sum(&|p| p.1) / n,
        sum(&|p| p.2) / n
    );
    let q = |f: f64| {
        let p = per[((per.len() - 1) as f64 * f) as usize];
        p.0 + p.1
    };
    println!(
        "per bot total B/s: p50 {:.0}, p99 {:.0}, max {:.0}; all bots {:.1} Mbit/s",
        q(0.5),
        q(0.99),
        q(1.0),
        sum(&|p| p.0 + p.1) * 8.0 / 1e6
    );
    let seen: Vec<usize> = counters
        .iter()
        .map(|c| c.seen.load(Ordering::Relaxed))
        .collect();
    let blind: Vec<String> = counters
        .iter()
        .enumerate()
        .filter(|(_, c)| c.seen.load(Ordering::Relaxed) == 0)
        .map(|(k, c)| format!("bot{k} ({} datagrams)", c.datagrams.load(Ordering::Relaxed)))
        .collect();
    if !blind.is_empty() {
        println!("bots seeing no entity: {}", blind.join(", "));
    }
    println!(
        "entities seen per bot: min {}, mean {:.0}, max {}; columns received in all {}; edits {} (refused {}); player states {}; errors {}; command datagrams {} (longest gap of a bot: max {} ms)",
        seen.iter().min().unwrap_or(&0),
        seen.iter().sum::<usize>() as f64 / n,
        seen.iter().max().unwrap_or(&0),
        after.iter().map(|a| a[3]).sum::<u64>(),
        counters
            .iter()
            .map(|c| c.edits.load(Ordering::Relaxed))
            .sum::<u64>(),
        counters
            .iter()
            .map(|c| c.refused.load(Ordering::Relaxed))
            .sum::<u64>(),
        counters
            .iter()
            .map(|c| c.states.load(Ordering::Relaxed))
            .sum::<u64>(),
        counters
            .iter()
            .map(|c| c.errors.load(Ordering::Relaxed))
            .sum::<u64>(),
        counters
            .iter()
            .map(|c| c.commands.load(Ordering::Relaxed))
            .sum::<u64>(),
        counters
            .iter()
            .map(|c| c.command_gap_ms.load(Ordering::Relaxed))
            .max()
            .unwrap_or(0),
    );
    for t in tasks {
        let _ = t.await;
    }
    Ok(())
}

async fn bot(
    k: usize,
    cli: &Cli,
    cert: &[u8],
    site: Option<&SiteKey>,
    c: &Counters,
    until: Instant,
) -> anyhow::Result<()> {
    let name = format!("swarm{k}");
    let auth = match site {
        Some(key) => Auth::Token(key.issue(&name, &cli.server_id, unix_ms(), MAX_TTL_MS)?),
        None => Auth::Secret("swarm-dev-secret".into()),
    };
    let mut conn = connect_with(cli.addr, cert, &name, auth).await?;
    let centre = match &cli.centre {
        Some(v) => [v[0], v[1], v[2]],
        None => conn.hello.spawn,
    };
    let granite = conn.blocks.id("granite").unwrap_or(1);
    // Granite without an inventory: god mode, granted to administrators only (others see
    // their placements refused, which still loads the server).
    conn.send(ClientMsg::God(true));
    let mut rng = Lcg(cli.seed ^ (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    // A random start within the radius, a random heading.
    let r = cli.radius * rng.next().sqrt();
    let a = rng.next() * std::f64::consts::TAU;
    let mut off = [r * a.cos(), r * a.sin()];
    let mut heading = rng.next() * std::f64::consts::TAU;
    let mut ents = Entities::new(0.05);
    let begin = Instant::now();
    let edit_every = cli.bots as f64 / cli.edits.max(1e-9);
    let mut next_edit = rng.next() * edit_every;
    let mut placed = false;
    // Where the server has the bot (its last state): it starts at the spawn and walks
    // towards a goal wandering in the disc.
    let mut at = conn.hello.spawn;
    let mut last_command: Option<u32> = None;
    let mut last_sent: Option<Instant> = None;
    let mut every = tokio::time::interval(Duration::from_millis(50));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    while Instant::now() < until {
        tokio::select! {
            _ = every.tick() => {
                // Walk; turn now and then; head back when outside the radius.
                heading += (rng.next() - 0.5) * 0.4;
                if off[0].hypot(off[1]) > cli.radius {
                    heading = (-off[1]).atan2(-off[0]);
                }
                off[0] += heading.cos() * cli.speed * 0.05;
                off[1] += heading.sin() * cli.speed * 0.05;
                let goal = [
                    torus::wrap_f64(centre[0] + off[0]),
                    centre[1],
                    torus::wrap_f64(centre[2] + off[1]),
                ];
                let d = [torus::delta_f64(at[0], goal[0]), torus::delta_f64(at[2], goal[2])];
                // Three steps towards the goal (heading from north towards east), jumping
                // over what stands in the way; the server moves the bot (chantier 5.3).
                let step = Input {
                    keys: if d[0].hypot(d[1]) > 1.0 { keys::FORWARD | keys::JUMP } else { 0 },
                    yaw: Input::quantize_yaw(d[0].atan2(-d[1])),
                    fly_speed: 0,
                };
                let last = last_command.map_or(2, |n| n + 3);
                last_command = Some(last);
                conn.send_commands(&Commands { last, pitch: quantize_pitch(0.0), inputs: vec![step; 3] });
                c.commands.fetch_add(1, Ordering::Relaxed);
                if let Some(t) = last_sent {
                    let gap = t.elapsed().as_millis() as u64;
                    c.command_gap_ms.fetch_max(gap, Ordering::Relaxed);
                }
                last_sent = Some(Instant::now());
                let pos = at;
                let t = begin.elapsed().as_secs_f64();
                if t >= next_edit && t > 5.0 {
                    next_edit += edit_every;
                    // Place a block three metres overhead, take it away the next time.
                    let at = [pos[0].floor() as i32, pos[1] as i32 + 3, pos[2].floor() as i32];
                    conn.send(ClientMsg::SetBlock { pos: at, block: if placed { 0 } else { granite }, cmd: last_command, slot: None });
                    placed = !placed;
                    c.edits.fetch_add(1, Ordering::Relaxed);
                }
            }
            ev = conn.events.recv() => match ev {
                Some(Event::Column(col)) => {
                    c.columns.fetch_add(1, Ordering::Relaxed);
                    drop(col);
                    c.column_bytes.store(conn.column_bytes.load(Ordering::Relaxed), Ordering::Relaxed);
                }
                Some(Event::Entities(d)) => {
                    c.entity_bytes.fetch_add(d.len() as u64, Ordering::Relaxed);
                    c.datagrams.fetch_add(1, Ordering::Relaxed);
                    match ents.receive(&d, begin.elapsed().as_secs_f64()) {
                        Ok(seq) => conn.send(ClientMsg::Ack { seqs: vec![seq] }),
                        Err(_) => { c.errors.fetch_add(1, Ordering::Relaxed); }
                    }
                    c.seen.store(ents.len(), Ordering::Relaxed);
                }
                Some(Event::Player(p)) => {
                    at = p.state.pos;
                    // Steps the server took without commands (a stall) are numbered after
                    // ours: the next commands follow its acknowledgement.
                    if p.ack > last_command {
                        last_command = p.ack;
                    }
                    c.states.fetch_add(1, Ordering::Relaxed);
                }
                Some(Event::Msg(ServerMsg::EditRefused { .. })) => {
                    c.refused.fetch_add(1, Ordering::Relaxed);
                }
                Some(Event::Closed(r)) => anyhow::bail!("closed: {r}"),
                Some(Event::Msg(_) | Event::Tile(..) | Event::Sky(..)) => {}
                None => anyhow::bail!("connection ended"),
            }
        }
    }
    conn.close();
    Ok(())
}

/// The server's certificate, with a hint when it is missing.
fn read_cert(path: &std::path::Path) -> anyhow::Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| {
        anyhow::anyhow!(
            "server certificate {}: {e} (copy the server's <data>/server/cert.der here)",
            path.display()
        )
    })
}
