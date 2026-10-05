//! The command-line interface.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Kippu (切符): ticketing infrastructure for conventions.
#[derive(Debug, Parser)]
#[command(version, propagate_version = true)]
pub struct Cli {
    /// Configuration file (TOML). Environment variables `KIPPU_<SECTION>__<KEY>` override it.
    #[arg(long, short, global = true, env = "KIPPU_CONFIG")]
    pub config: Option<PathBuf>,

    /// Log output format.
    #[arg(
        long,
        global = true,
        env = "KIPPU_LOG_FORMAT",
        default_value = "pretty"
    )]
    pub log_format: LogFormat,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// How logs are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
    /// Human-readable lines.
    Pretty,
    /// One JSON object per line, for log collectors.
    Json,
}

/// Subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Serve the HTTP API (and, by default, run background workers).
    Serve(ServeArgs),
    /// Run background workers only, without the HTTP API.
    Worker(DatabaseArgs),
    /// Apply database migrations and exit.
    Migrate(DatabaseArgs),
    /// Generate an Ed25519 key pair for the configuration.
    Keygen {
        /// Print PEM (PKCS#8 / SPKI) instead of base64.
        #[arg(long)]
        pem: bool,
    },
    /// Mint a short-lived root token with a root private key.
    RootToken(RootTokenArgs),
    /// Inspect configuration.
    #[command(subcommand)]
    Config(ConfigCommand),
}

/// Overrides for where the database is.
#[derive(Debug, Clone, clap::Args)]
pub struct DatabaseArgs {
    /// Database URL, e.g. `sqlite:///var/lib/kippu.db`. Overrides `database.url`.
    #[arg(long)]
    pub database_url: Option<String>,
}

/// Arguments of `serve`.
#[derive(Debug, Clone, clap::Args)]
pub struct ServeArgs {
    /// Address to listen on. Overrides `server.listen`.
    #[arg(long)]
    pub listen: Option<SocketAddr>,
    /// Serve HTTP only; run workers elsewhere with `kippu worker`.
    #[arg(long)]
    pub no_workers: bool,
    /// Where the database is.
    #[command(flatten)]
    pub database: DatabaseArgs,
}

/// Arguments of `root-token`.
#[derive(Debug, Clone, clap::Args)]
pub struct RootTokenArgs {
    /// File holding the root private key (base64 seed or PKCS#8 PEM). `-` reads stdin.
    #[arg(long)]
    pub key: PathBuf,
    /// The key's name, as configured under `root.keys`.
    #[arg(long)]
    pub name: String,
    /// The deployment's `issuer.id`. Read from the configuration if omitted.
    #[arg(long)]
    pub audience: Option<String>,
    /// Lifetime in seconds (at most the server's `root.max_token_ttl_seconds`).
    #[arg(long, default_value_t = 600)]
    pub ttl: u32,
}

/// `config` subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the effective configuration, with secrets redacted.
    Check(DatabaseArgs),
}
