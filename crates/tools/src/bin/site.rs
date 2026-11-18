//! Stand-in for the site's token service (chantier 2): makes the site's Ed25519 key and
//! issues login tokens with it. The server only holds the public key (`site_public_key`).
//!
//! ```text
//! cargo run -p nascio-tools --bin site -- keygen out/site.key
//! cargo run -p nascio-tools --bin site -- token out/site.key --name alice --server dev
//! ```

use std::path::PathBuf;

use anyhow::{Context, ensure};
use clap::{Parser, Subcommand};
use nascio_server::accounts::{check_name, parse_duration};
use nascio_server::net::unix_ms;
use nascio_server::token::{MAX_TTL_MS, SiteKey, b64_encode, generate_key};

#[derive(Parser)]
#[command(
    name = "site",
    about = "Nascio site stand-in: signing key and login tokens"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Writes a new private key (keep it secret) and prints the public key for the servers.
    Keygen {
        out: PathBuf,
        /// Replaces an existing key.
        #[arg(long)]
        force: bool,
    },
    /// Prints the public key of a private key.
    Public { key: PathBuf },
    /// Prints a token letting `name` log in to the server `server` (its `server_id`).
    Token {
        key: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        server: String,
        /// Validity: 30s, 5m… (at most 5 minutes, which servers enforce).
        #[arg(long, default_value = "5m")]
        ttl: String,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Keygen { out, force } => {
            ensure!(
                force || !out.exists(),
                "{} exists (--force to replace it)",
                out.display()
            );
            let (pkcs8, public) = generate_key()?;
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&out, b64_encode(&pkcs8) + "\n")
                .with_context(|| format!("writing {}", out.display()))?;
            println!("private key written to {} — keep it secret", out.display());
            println!("site_public_key = \"{}\"", b64_encode(&public));
        }
        Cmd::Public { key } => {
            let key = SiteKey::load(&key)?;
            println!("site_public_key = \"{}\"", b64_encode(&key.public_key()));
        }
        Cmd::Token {
            key,
            name,
            server,
            ttl,
        } => {
            check_name(&name)?;
            let ttl = parse_duration(&ttl).context("ttl: 30s, 5m…")?;
            ensure!(
                ttl <= MAX_TTL_MS,
                "servers refuse tokens valid over 5 minutes"
            );
            let key = SiteKey::load(&key)?;
            println!("{}", key.issue(&name, &server, unix_ms(), ttl)?);
        }
    }
    Ok(())
}
