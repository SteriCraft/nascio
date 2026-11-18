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
use nascio_server::{RunningServer, ServerConfig, accounts};
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
    /// Development: every account may log in with a secret, and an unknown name creates its
    /// account (`swarm`, the viewer without a token).
    #[arg(long)]
    open_registration: bool,
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
    if cli.open_registration {
        cfg.open_registration = true;
        cfg.secret_login = nascio_server::config::SecretLogin::All;
    }
    let world = World::load(&cli.world)?;
    let data = cli.data.clone().unwrap_or_else(|| cli.world.clone());
    let server = RunningServer::start(&world, cfg, &data)?;
    println!(
        "listening on {} — certificate {}",
        server.addr,
        data.join("server").join("cert.der").display()
    );
    let c = &server.shared.cfg;
    match &server.shared.tokens {
        Some(_) => println!("site tokens accepted for server_id \"{}\"", c.server_id),
        None => println!("no site_public_key: site tokens refused"),
    }
    println!(
        "secret logins: {:?}{}",
        c.secret_login,
        if c.open_registration {
            ", open registration"
        } else {
            ""
        }
    );
    // Administration console: one command per line on the standard input.
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::Builder::new()
        .name("console".into())
        .spawn(move || {
            for line in std::io::stdin().lines() {
                let Ok(line) = line else { break };
                if cmd_tx.send(line).is_err() {
                    break;
                }
            }
        })?;
    println!("console: {HELP}");
    let mut every = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            Some(line) = cmd_rx.recv() => {
                if !command(&server, line.trim()) {
                    break;
                }
            }
            _ = every.tick() => print_stats(&server),
        }
    }
    println!("stopping");
    server.shutdown()
}

fn print_stats(server: &RunningServer) {
    let s = server
        .stats
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let sh = &server.shared;
    let load = |a: &std::sync::atomic::AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed);
    println!(
        "players {} · tick p50 {:.2} ms p99 {:.2} ms (replication p99 {:.2} ms) · edits {} (refused {}) · columns built {} sent {} ({:.1} MB) · entities {} datagrams ({:.1} MB, {} skipped) · abuse events {}",
        s.players,
        s.tick_ms(0.5),
        s.tick_ms(0.99),
        s.repl_ms(0.99),
        s.edits,
        s.refused,
        load(&sh.cache.built),
        load(&sh.sent_columns),
        load(&sh.sent_bytes) as f64 / 1e6,
        s.entity_datagrams,
        s.entity_bytes as f64 / 1e6,
        s.entity_skipped,
        load(&sh.abuse.events),
    );
}

const HELP: &str = "help · list · kick <name> [reason] · ban <name> [30m|12h|7d|2w] [reason] · unban <name> · bans · admin <name> on|off · stats · adduser <name> <secret> [admin] · stop";

/// Runs a console command; false to stop the server.
fn command(server: &RunningServer, line: &str) -> bool {
    let mut words = line.split_whitespace();
    let sh = &server.shared;
    match words.next() {
        None => {}
        Some("help") => println!("{HELP}"),
        Some("list") => {
            let players = sh.list();
            println!("{} players online", players.len());
            for (name, id, p) in players {
                println!("  {name} (#{id}) at {:.0}, {:.0}, {:.0}", p[0], p[1], p[2]);
            }
        }
        Some("kick") => match words.next() {
            Some(name) => {
                let reason: Vec<&str> = words.collect();
                let reason = if reason.is_empty() {
                    "kicked by the administrator".to_owned()
                } else {
                    reason.join(" ")
                };
                if sh.kick(name, &reason) {
                    println!("{name} kicked");
                } else {
                    println!("{name} is not online");
                }
            }
            None => println!("kick <name> [reason]"),
        },
        Some("ban") => match words.next() {
            Some(name) => {
                let mut rest: Vec<&str> = words.collect();
                let now = nascio_server::net::unix_ms();
                // A first word that reads as a duration is one; otherwise the ban is permanent.
                let until = match rest.first().and_then(|w| accounts::parse_duration(w)) {
                    Some(d) => {
                        rest.remove(0);
                        Some(now.saturating_add(d))
                    }
                    None => None,
                };
                match sh.ban(name, until, &rest.join(" ")) {
                    Ok((ban, kicked)) => {
                        println!("{name} {}", ban.message(now));
                        if kicked {
                            println!("{name} kicked");
                        }
                    }
                    Err(e) => println!("refused: {e:#}"),
                }
            }
            None => println!("ban <name> [30m|12h|7d|2w] [reason]"),
        },
        Some("unban") => match words.next() {
            Some(name) => match sh.accounts.unban(name, nascio_server::net::unix_ms()) {
                Ok(true) => println!("{name} unbanned"),
                Ok(false) => println!("{name} was not banned"),
                Err(e) => println!("refused: {e:#}"),
            },
            None => println!("unban <name>"),
        },
        Some("bans") => {
            let now = nascio_server::net::unix_ms();
            let bans = sh.accounts.bans(now);
            println!("{} accounts banned", bans.len());
            for (name, ban) in bans {
                println!("  {name}: {}", ban.message(now));
            }
        }
        Some("admin") => match (words.next(), words.next()) {
            (Some(name), Some(flag @ ("on" | "off"))) => {
                match sh.accounts.set_admin(name, flag == "on") {
                    Ok(()) => println!("{name}: admin {flag}"),
                    Err(e) => println!("refused: {e:#}"),
                }
            }
            _ => println!("admin <name> on|off"),
        },
        Some("stats") => print_stats(server),
        Some("adduser") => match (words.next(), words.next(), words.next()) {
            (Some(name), Some(secret), flag @ (None | Some("admin"))) => {
                let admin = flag.is_some();
                match sh
                    .accounts
                    .create(name, secret, nascio_server::net::unix_ms())
                    .and_then(|()| sh.accounts.set_admin(name, admin))
                {
                    Ok(()) if admin => println!("administrator {name} created"),
                    Ok(()) => println!("account {name} created"),
                    Err(e) => println!("refused: {e:#}"),
                }
            }
            _ => println!("adduser <name> <secret> [admin]"),
        },
        Some("stop") => return false,
        Some(c) => println!("unknown command {c} (help)"),
    }
    true
}
