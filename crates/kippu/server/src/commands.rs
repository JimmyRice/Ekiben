//! What each subcommand does.

use std::io::Read;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};
use kippu_core::auth::tokens::mint_root_token;
use kippu_core::keys::parse_signing_key;
use kippu_core::{Clock, Config, Module, SystemClock};
use kippu_domain::Duration;
use kippu_store::BoxError;
use secrecy::{ExposeSecret, SecretString};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::cli::{Cli, DatabaseArgs, RootTokenArgs, ServeArgs};
use crate::{adapters, assemble, config, serve_app};

fn load(
    cli: &Cli,
    database: &DatabaseArgs,
    listen: Option<std::net::SocketAddr>,
) -> Result<Config, BoxError> {
    let figment = config::sources(cli.config.as_deref());
    let figment = config::with_override(figment, "database.url", database.database_url.clone());
    let figment = config::with_override(figment, "server.listen", listen);
    Ok(config::extract(&figment)?)
}

/// Resolves when the process is asked to stop (Ctrl-C, or SIGTERM on Unix).
async fn shutdown_signal(shutdown: CancellationToken) {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
    shutdown.cancel();
}

pub(crate) async fn serve(
    cli: &Cli,
    args: &ServeArgs,
    modules: Vec<Arc<dyn Module>>,
) -> Result<(), BoxError> {
    let config = load(cli, &args.database, args.listen)?;
    let listen = config.server.listen;
    let workers = config.workers.enabled && !args.no_workers;
    let app = assemble(config, modules).await?;
    let listener = TcpListener::bind(listen).await?;
    tracing::info!(address = %listener.local_addr()?, workers, "kippu is serving");
    let shutdown = CancellationToken::new();
    tokio::spawn(shutdown_signal(shutdown.clone()));
    serve_app(&app, listener, shutdown, workers).await?;
    Ok(())
}

pub(crate) async fn worker(
    cli: &Cli,
    args: &DatabaseArgs,
    modules: Vec<Arc<dyn Module>>,
) -> Result<(), BoxError> {
    let config = load(cli, args, None)?;
    let app = assemble(config, modules).await?;
    let shutdown = CancellationToken::new();
    tokio::spawn(shutdown_signal(shutdown.clone()));
    let tracker = tokio_util::task::TaskTracker::new();
    app.spawn_tasks(&tracker, &shutdown);
    tracker.close();
    tracing::info!(tasks = app.tasks().len(), "kippu worker is running");
    tracker.wait().await;
    Ok(())
}

pub(crate) async fn migrate(cli: &Cli, args: &DatabaseArgs) -> Result<(), BoxError> {
    let config = load(cli, args, None)?;
    let store = adapters::connect(config.database.url.expose_secret()).await?;
    store.migrate().await?;
    tracing::info!(backend = store.capabilities().backend, "migrations applied");
    Ok(())
}

#[expect(
    clippy::print_stdout,
    reason = "printing the key pair is the command's output"
)]
pub(crate) fn keygen(pem: bool) -> Result<(), BoxError> {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).map_err(|error| error.to_string())?;
    let key = SigningKey::from_bytes(&seed);
    if pem {
        print!("{}", key.to_pkcs8_pem(LineEnding::LF)?.as_str());
        print!("{}", key.verifying_key().to_public_key_pem(LineEnding::LF)?);
    } else {
        println!("private key: {}", STANDARD.encode(key.to_bytes()));
        println!(
            "public key:  {}",
            STANDARD.encode(key.verifying_key().as_bytes())
        );
    }
    Ok(())
}

#[expect(
    clippy::print_stdout,
    reason = "printing the token is the command's output"
)]
pub(crate) fn root_token(cli: &Cli, args: &RootTokenArgs) -> Result<(), BoxError> {
    let audience = match &args.audience {
        Some(audience) => audience.clone(),
        None => config::sources(cli.config.as_deref())
            .extract_inner::<String>("issuer.id")
            .unwrap_or_else(|_| kippu_core::config::IssuerConfig::default().id),
    };
    let mut key_text = String::new();
    if args.key.as_os_str() == "-" {
        std::io::stdin().read_to_string(&mut key_text)?;
    } else {
        key_text = std::fs::read_to_string(&args.key)?;
    }
    let key = parse_signing_key("--key", &SecretString::from(key_text))?;
    let ttl = Duration::seconds(i64::from(args.ttl));
    println!(
        "{}",
        mint_root_token(&key, &args.name, &audience, SystemClock.now(), ttl)
    );
    Ok(())
}

#[expect(
    clippy::print_stdout,
    reason = "printing the configuration is the command's output"
)]
pub(crate) fn config_check(cli: &Cli, args: &DatabaseArgs) -> Result<(), BoxError> {
    let config = load(cli, args, None)?;
    println!("{config:#?}");
    Ok(())
}
