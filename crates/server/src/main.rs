//! `nascio-server [world dir]`: serves a generated world. Deltas live in
//! `<data>/deltas/`, the certificate in `<data>/server/` (clients pin it); `<data>` is the
//! world folder unless `--data` says otherwise.
//!
//! With its window (feature `gui`, on by default), the server opens on a start dialog: an
//! existing world or a new one (seed, folder next to the executable), and the options of the
//! command line, filled from it and from the last start (`server-launch.toml` next to the
//! executable). Then the window shows the map with the players, the console, the
//! measurements and the players online; the terminal's console works too. `--nogui` runs
//! without a window and needs the world folder.
//!
//! ```text
//! cargo run -p nascio-server --release
//! cargo run -p nascio-server --release -- out/server/world-42 --data out/server/data --nogui
//! ```

#[cfg(feature = "gui")]
mod gui;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use nascio_server::net::Shared;
use nascio_server::sim::SimStats;
use nascio_server::{RunningServer, ServerConfig, accounts, say};
use nascio_world::world_io::World;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Parser)]
#[command(name = "nascio-server", about = "Nascio authoritative server")]
struct Cli {
    /// World folder (with the window: proposed in the start dialog).
    world: Option<PathBuf>,
    /// TOML settings (`ServerConfig`).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Folder of the deltas and the certificate (default: the world folder).
    #[arg(long)]
    data: Option<PathBuf>,
    /// Overrides `bind` (default 127.0.0.1:7777; 0.0.0.0:7777 to accept other machines,
    /// which needs a `server_id` of its own).
    #[arg(long)]
    bind: Option<SocketAddr>,
    /// Overrides `server_id` (the audience of the site's tokens).
    #[arg(long)]
    server_id: Option<String>,
    /// Development: every account may log in with a secret, and an unknown name creates its
    /// account (`swarm`, the viewer without a token).
    #[arg(long)]
    open_registration: bool,
    /// One spawn point x,z for every player (load tests with `swarm`), instead of a point per
    /// account near the world's spawn sites.
    #[arg(long, value_delimiter = ',')]
    spawn_xz: Option<Vec<f64>>,
    /// No window: the console on the terminal only (a server without a display). Without
    /// the `gui` feature, there is never a window.
    #[arg(long)]
    nogui: bool,
}

/// How to start the server: the command line's options, or the start dialog's.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub config: Option<PathBuf>,
    pub data: Option<PathBuf>,
    pub bind: Option<SocketAddr>,
    pub server_id: Option<String>,
    pub open_registration: bool,
    pub spawn_xz: Option<[f64; 2]>,
}

impl Options {
    /// The server's settings: the TOML file, then the overrides.
    fn config(&self) -> anyhow::Result<ServerConfig> {
        let mut cfg = match &self.config {
            Some(p) => ServerConfig::load(p).with_context(|| format!("reading {}", p.display()))?,
            None => ServerConfig::default(),
        };
        if let Some(b) = self.bind {
            cfg.bind = b;
        }
        if let Some(id) = &self.server_id {
            cfg.server_id = id.clone();
        }
        if self.open_registration {
            cfg.open_registration = true;
            cfg.secret_login = nascio_server::config::SecretLogin::All;
        }
        if let Some(p) = self.spawn_xz {
            cfg.spawn_xz = Some(p);
        }
        Ok(cfg)
    }
}

/// A started server, seen from outside its console task.
pub struct Running {
    /// Console commands (the window's, `stop`).
    pub commands: UnboundedSender<String>,
    /// The console task; it ends once the server stopped.
    pub console: tokio::task::JoinHandle<anyhow::Result<()>>,
    pub shared: Arc<Shared>,
    pub stats: Arc<std::sync::Mutex<SimStats>>,
    pub addr: SocketAddr,
    /// Set once the server stopped (`stop`, Ctrl+C).
    pub done: Arc<AtomicBool>,
}

/// Starts the server on `world` (folder `world_dir`) and its console task, which reads the
/// standard input too. `periodic`: print the measurements every 10 s (no window). Call from
/// outside the runtime.
pub fn launch(
    opts: &Options,
    world: &World,
    world_dir: &Path,
    rt: &tokio::runtime::Handle,
    periodic: bool,
) -> anyhow::Result<Running> {
    let cfg = opts.config()?;
    let data = opts.data.clone().unwrap_or_else(|| world_dir.to_owned());
    let server = rt.block_on(async { RunningServer::start(world, cfg, &data) })?;
    say!(
        "world {} (seed {}) — listening on {} — certificate {}",
        world_dir.display(),
        world.config.seed,
        server.addr,
        data.join("server").join("cert.der").display()
    );
    let c = &server.shared.cfg;
    match &server.shared.tokens {
        Some(_) => say!("site tokens accepted for server_id \"{}\"", c.server_id),
        None => say!("no site_public_key: site tokens refused"),
    }
    say!(
        "secret logins: {:?}{}",
        c.secret_login,
        if c.open_registration {
            ", open registration"
        } else {
            ""
        }
    );
    let sp = &server.shared.spawns;
    match sp.fixed {
        Some(p) => say!("every player spawns at {}, {}", p.x, p.z),
        None => say!(
            "{} spawn sites (console: spawns), points within {} m",
            sp.sites.len(),
            c.spawn_radius_m
        ),
    }
    // Administration console: one command per line on the standard input, or from the
    // window's console.
    let (commands, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let stdin_tx = commands.clone();
    std::thread::Builder::new()
        .name("console".into())
        .spawn(move || {
            for line in std::io::stdin().lines() {
                let Ok(line) = line else { break };
                if stdin_tx.send(line).is_err() {
                    break;
                }
            }
        })?;
    let done = Arc::new(AtomicBool::new(false));
    let (shared, stats, addr) = (server.shared.clone(), server.stats.clone(), server.addr);
    let console = rt.spawn(console(server, cmd_rx, periodic, done.clone()));
    Ok(Running {
        commands,
        console,
        shared,
        stats,
        addr,
        done,
    })
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let spawn_xz = match cli.spawn_xz.as_deref() {
        None => None,
        Some([x, z]) => Some([*x, *z]),
        Some(_) => anyhow::bail!("--spawn-xz takes x,z"),
    };
    let opts = Options {
        config: cli.config.clone(),
        data: cli.data.clone(),
        bind: cli.bind,
        server_id: cli.server_id.clone(),
        open_registration: cli.open_registration,
        spawn_xz,
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    #[cfg(feature = "gui")]
    if !cli.nogui {
        // The window starts the server (start dialog) and hands it back here once closed.
        let started = Arc::new(std::sync::Mutex::new(None::<Running>));
        gui::run(gui::Launch {
            world: cli.world.clone(),
            opts,
            rt: rt.handle().clone(),
            started: started.clone(),
        });
        let running = started.lock().unwrap_or_else(|e| e.into_inner()).take();
        return match running {
            Some(r) => {
                // The window was closed: stop the server (a no-op if `stop` closed it).
                let _ = r.commands.send("stop".into());
                rt.block_on(r.console)?
            }
            None => Ok(()),
        };
    }
    let dir = cli
        .world
        .context("the world folder is needed without the window (--nogui)")?;
    let world = World::load(&dir)?;
    let r = launch(&opts, &world, &dir, rt.handle(), true)?;
    rt.block_on(r.console)?
}

/// Runs the console's commands until `stop` or Ctrl+C, then stops the server. `periodic`:
/// print the measurements every 10 s (the window draws them instead).
async fn console(
    server: RunningServer,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<String>,
    periodic: bool,
    done: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let term = Term::detect();
    say!("console: type a command, or help (help <command> for its details)");
    term.start();
    let mut every = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            Some(line) = commands.recv() => {
                let line = line.trim();
                if !line.is_empty() && !periodic {
                    // The window's console shows what was typed.
                    nascio_server::console::out(format!("> {line}"));
                }
                if !command(&server, line) {
                    break;
                }
                term.prompt();
            }
            _ = every.tick(), if periodic => {
                term.interrupt();
                print_stats(&server);
                term.prompt();
            }
        }
    }
    term.end();
    say!("stopping");
    let r = server.shutdown();
    done.store(true, Ordering::Relaxed);
    r
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
    say!(
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
        "seed",
        "",
        "the world's seed",
        "Prints the seed of the world served (the same seed and generator version make the same world).",
    ),
    (
        "spawns",
        "",
        "spawn sites",
        "Lists the world's spawn sites (x, z; drawn from the seed at the first start, kept in server/spawn_sites.toml) and the accounts whose point lies near each. A new account gets a point drawn at random within spawn_radius_m of a site drawn at random, kept for good.",
    ),
    (
        "message",
        "<text>",
        "writes to every player",
        "Every player online reads [Admin]: <text> in their chat.",
    ),
    (
        "tell",
        "<name> <text>",
        "writes to one player",
        "Only that player reads [Admin]: <text> in their chat.",
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
                say!("  {call:<34} {summary}");
            }
        }
        Some(t) => match COMMANDS.iter().find(|c| c.0 == t) {
            Some((name, args, summary, details)) => {
                say!("{name} {args}");
                say!("  {summary}.");
                say!("  {details}");
            }
            None => say!("no command {t} (help lists them)"),
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
            say!("{} players online", players.len());
            for (name, id, p) in players {
                say!("  {name} (#{id}) at {:.0}, {:.0}, {:.0}", p[0], p[1], p[2]);
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
                    say!("{name} kicked");
                } else {
                    say!("{name} is not online");
                }
            }
            None => say!("kick <name> [reason]"),
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
                        say!("{name} {}", ban.message(now));
                        if kicked {
                            say!("{name} kicked");
                        }
                    }
                    Err(e) => say!("refused: {e:#}"),
                }
            }
            None => say!("ban <name> [30m|12h|7d|2w] [reason]"),
        },
        Some("unban") => match words.next() {
            Some(name) => match sh.accounts.unban(name, nascio_server::net::unix_ms()) {
                Ok(true) => say!("{name} unbanned"),
                Ok(false) => say!("{name} was not banned"),
                Err(e) => say!("refused: {e:#}"),
            },
            None => say!("unban <name>"),
        },
        Some("bans") => {
            let now = nascio_server::net::unix_ms();
            let bans = sh.accounts.bans(now);
            say!("{} accounts banned", bans.len());
            for (name, ban) in bans {
                say!("  {name}: {}", ban.message(now));
            }
        }
        Some("admin") => match (words.next(), words.next()) {
            (Some(name), Some(flag @ ("on" | "off"))) => {
                match sh.accounts.set_admin(name, flag == "on") {
                    Ok(()) => say!("{name}: admin {flag}"),
                    Err(e) => say!("refused: {e:#}"),
                }
            }
            _ => say!("admin <name> on|off"),
        },
        Some("stats") => print_stats(server),
        Some("seed") => say!("seed {}", sh.seed),
        Some("spawns") => {
            let sp = &sh.spawns;
            if let Some(p) = sp.fixed {
                say!("every player spawns at {}, {} (spawn_xz)", p.x, p.z);
            } else {
                let count = sh.accounts.per_site(sp.sites.len());
                for (i, (p, n)) in sp.sites.iter().zip(count).enumerate() {
                    say!("  site {i}: {}, {} ({n} accounts)", p.x, p.z);
                }
            }
        }
        Some("adduser") => match (words.next(), words.next(), words.next()) {
            (Some(name), Some(secret), flag @ (None | Some("admin"))) => {
                let admin = flag.is_some();
                match sh
                    .accounts
                    .create(name, secret, nascio_server::net::unix_ms())
                    .and_then(|()| sh.accounts.set_admin(name, admin))
                {
                    Ok(()) if admin => say!("administrator {name} created"),
                    Ok(()) => say!("account {name} created"),
                    Err(e) => say!("refused: {e:#}"),
                }
            }
            _ => say!("adduser <name> <secret> [admin]"),
        },
        Some("message") => {
            let text: Vec<&str> = words.collect();
            if text.is_empty() {
                say!("message <text>");
            } else {
                sh.announce(&text.join(" "));
                say!("[Admin]: {}", text.join(" "));
            }
        }
        Some("tell") => match words.next() {
            Some(name) => {
                let text: Vec<&str> = words.collect();
                if text.is_empty() {
                    say!("tell <name> <text>");
                } else if sh.tell(name, &text.join(" ")) {
                    say!("[Admin → {name}]: {}", text.join(" "));
                } else {
                    say!("{name} is not online");
                }
            }
            None => say!("tell <name> <text>"),
        },
        Some("stop") => return false,
        Some(c) => say!("unknown command {c} (help)"),
    }
    true
}
