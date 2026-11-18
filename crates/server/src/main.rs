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
    /// Overrides `bind` (default 127.0.0.1:7777; 0.0.0.0:7777 to accept other machines,
    /// which needs a `server_id` of its own).
    #[arg(long)]
    bind: Option<std::net::SocketAddr>,
    /// Overrides `server_id` (the audience of the site's tokens).
    #[arg(long)]
    server_id: Option<String>,
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
    if let Some(id) = cli.server_id.clone() {
        cfg.server_id = id;
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
    let term = Term::detect();
    println!("console: type a command, or help (help <command> for its details)");
    term.start();
    let mut every = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            Some(line) = cmd_rx.recv() => {
                if !command(&server, line.trim()) {
                    break;
                }
                term.prompt();
            }
            _ = every.tick() => {
                term.interrupt();
                print_stats(&server);
                term.prompt();
            }
        }
    }
    term.end();
    println!("stopping");
    server.shutdown()
}

/// The console's look on a terminal: a `>` prompt and a blinking underline cursor, which
/// read together as `>_`. Off when the output is not a terminal (a log file, a pipe).
struct Term {
    /// The output is a terminal.
    tty: bool,
    /// It understands ANSI sequences (cursor shape, clearing the line).
    ansi: bool,
}

impl Term {
    fn detect() -> Self {
        use std::io::IsTerminal;
        let tty = std::io::stdout().is_terminal() && std::io::stdin().is_terminal();
        // Windows: Windows Terminal and VS Code process ANSI sequences; the old console
        // window would print them.
        let ansi = tty
            && (!cfg!(windows)
                || std::env::var_os("WT_SESSION").is_some()
                || std::env::var_os("TERM_PROGRAM").is_some());
        Self { tty, ansi }
    }

    fn write(&self, s: &str) {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
    }

    /// Shows the prompt (and the blinking underline cursor).
    fn start(&self) {
        if self.ansi {
            self.write("\x1b[3 q");
        }
        self.prompt();
    }

    fn prompt(&self) {
        if self.tty {
            self.write("> ");
        }
    }

    /// Before a line printed while the prompt waits: clear the prompt's line.
    fn interrupt(&self) {
        if self.ansi {
            self.write("\r\x1b[K");
        } else if self.tty {
            self.write("\n");
        }
    }

    /// Gives the terminal its own cursor back.
    fn end(&self) {
        if self.ansi {
            self.write("\r\x1b[K\x1b[0 q");
        } else if self.tty {
            self.write("\n");
        }
    }
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

/// The console commands: name, arguments, summary, details.
const COMMANDS: &[(&str, &str, &str, &str)] = &[
    (
        "help",
        "[command]",
        "lists the commands, or details one",
        "Without an argument, lists every command with its arguments. With a command's name, tells what it does.",
    ),
    (
        "list",
        "",
        "players online",
        "Prints each player online: account name, entity number and position (x, y, z in metres).",
    ),
    (
        "stats",
        "",
        "server measurements",
        "Prints players, tick times (median and 99th percentile), edits, columns built and sent, entity datagrams and abuse events. Printed every 10 s anyway.",
    ),
    (
        "kick",
        "<name> [reason]",
        "disconnects a player",
        "Tells the player the reason (default: kicked by the administrator), then closes the connection. The player may log in again: see ban.",
    ),
    (
        "ban",
        "<name> [duration] [reason]",
        "bans an account",
        "Duration: 30s, 30m, 12h, 7d or 2w; without one, the ban is permanent (a first word that is not a duration starts the reason). Kicks the player if online; a login is refused with the reason and the time left. Kept in accounts.toml; a new ban replaces the former one.",
    ),
    (
        "unban",
        "<name>",
        "lifts a ban",
        "The account may log in again at once.",
    ),
    (
        "bans",
        "",
        "bans in force",
        "Lists the banned accounts with the time left and the reason; expired bans are not listed.",
    ),
    (
        "adduser",
        "<name> <secret> [admin]",
        "creates an account",
        "Name: 3 to 16 letters, digits or _; secret: 8 characters at least (avoid spaces). With admin, the account may log in with its secret (secret_login = \"admin\"); players get their accounts from the site's tokens.",
    ),
    (
        "admin",
        "<name> on|off",
        "makes an account an administrator, or not",
        "Administrators may log in with their secret when secret_login = \"admin\" (the default), for instance while the site is down.",
    ),
    (
        "stop",
        "",
        "stops the server",
        "Closes the connections and writes every edit to the disk (Ctrl+C does the same).",
    ),
];

fn help(topic: Option<&str>) {
    match topic {
        None => {
            for (name, args, summary, _) in COMMANDS {
                let call = format!("{name} {args}");
                println!("  {call:<34} {summary}");
            }
        }
        Some(t) => match COMMANDS.iter().find(|c| c.0 == t) {
            Some((name, args, summary, details)) => {
                println!("{name} {args}");
                println!("  {summary}.");
                println!("  {details}");
            }
            None => println!("no command {t} (help lists them)"),
        },
    }
}

/// Runs a console command; false to stop the server.
fn command(server: &RunningServer, line: &str) -> bool {
    let mut words = line.split_whitespace();
    let sh = &server.shared;
    match words.next() {
        None => {}
        Some("help") => help(words.next()),
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
