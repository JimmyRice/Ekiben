//! The command-line interface.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use kippu_domain::Timestamp;

/// Kippu (切符): ticketing infrastructure for conventions.
#[derive(Debug, Parser)]
#[command(name = "kippu", version, propagate_version = true)]
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
    /// A block per request, from arrival to response, for people.
    Pretty,
    /// One line per request.
    Compact,
    /// One JSON object per event, tagged with `request_id`, for log collectors.
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
    Keygen(KeygenArgs),
    /// Mint a short-lived root token with a root private key.
    RootToken(RootTokenArgs),
    /// Check a ticket offline, the way a gate does.
    Verify(VerifyArgs),
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

/// Arguments of `keygen`.
#[derive(Debug, Clone, clap::Args)]
pub struct KeygenArgs {
    /// Use PEM (PKCS#8 private key, SPKI public key) instead of base64.
    #[arg(long)]
    pub pem: bool,
    /// Write `<PREFIX>.key` (private, mode 0600) and `<PREFIX>.pub` instead of printing.
    /// Existing files are never overwritten.
    #[arg(long, value_name = "PREFIX")]
    pub out: Option<PathBuf>,
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

/// Arguments of `verify`.
#[derive(Debug, Clone, clap::Args)]
pub struct VerifyArgs {
    /// The ticket: text in `--encoding`, a file holding it (text or raw bytes, e.g. saved from
    /// the API), or `-` for stdin.
    pub ticket: String,
    /// A trusted ticket public key: base64 (as listed by `/.well-known/kippu/ticket-keys`),
    /// PEM, or a file holding one (such as `ticket.pub`). Repeat for retired keys.
    #[arg(long = "key", required = true, value_name = "KEY")]
    pub keys: Vec<String>,
    /// Also require this `issuer` claim, i.e. the deployment's `issuer.id`.
    #[arg(long)]
    pub issuer: Option<String>,
    /// How the ticket is encoded. `auto` recognizes all of them by the ticket header.
    #[arg(long, default_value = "auto")]
    pub encoding: TicketEncoding,
    /// Check the validity window at this instant (RFC 3339) instead of now.
    #[arg(long, value_name = "TIME")]
    pub at: Option<Timestamp>,
}

/// How a ticket handed to `verify` is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TicketEncoding {
    /// Whichever of the others yields a ticket header.
    Auto,
    /// Standard or URL-safe base64, with or without padding.
    Base64,
    /// Base45 (RFC 9285), the QR alphanumeric-mode text form.
    Base45,
    /// Hexadecimal.
    Hex,
    /// The raw bytes.
    Binary,
}

/// `config` subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the effective configuration, with secrets redacted.
    Check(DatabaseArgs),
}
