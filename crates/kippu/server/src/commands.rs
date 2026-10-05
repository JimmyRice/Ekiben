//! What each subcommand does.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::spki::der::zeroize::Zeroizing;
use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};
use kippu_core::auth::tokens::mint_root_token;
use kippu_core::keys::parse_signing_key;
use kippu_core::{Clock, Config, Module, SystemClock};
use kippu_domain::Duration;
use kippu_store::BoxError;
use secrecy::{ExposeSecret, SecretString};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::cli::{Cli, DatabaseArgs, KeygenArgs, RootTokenArgs, ServeArgs};
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
pub(crate) fn keygen(args: &KeygenArgs) -> Result<(), BoxError> {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).map_err(|error| error.to_string())?;
    let key = SigningKey::from_bytes(&seed);
    let (private, public) = if args.pem {
        (
            key.to_pkcs8_pem(LineEnding::LF)?,
            key.verifying_key().to_public_key_pem(LineEnding::LF)?,
        )
    } else {
        (
            Zeroizing::new(format!("{}\n", STANDARD.encode(key.to_bytes()))),
            format!("{}\n", STANDARD.encode(key.verifying_key().as_bytes())),
        )
    };
    let Some(prefix) = &args.out else {
        if args.pem {
            print!("{}{public}", private.as_str());
        } else {
            print!("private key: {}public key:  {public}", private.as_str());
        }
        return Ok(());
    };
    let private_path = with_suffix(prefix, "key");
    let public_path = with_suffix(prefix, "pub");
    // Check both first so a refusal leaves nothing half-written.
    for path in [&private_path, &public_path] {
        if path.exists() {
            return Err(format!("{} already exists; not overwriting it", path.display()).into());
        }
    }
    write_new(&private_path, private.as_bytes(), 0o600)?;
    write_new(&public_path, public.as_bytes(), 0o644)?;
    println!("private key: {}", private_path.display());
    println!("public key:  {}", public_path.display());
    Ok(())
}

/// `prefix` with `.suffix` appended (not replacing an extension: `keys/root.v2` → `root.v2.key`).
fn with_suffix(prefix: &Path, suffix: &str) -> PathBuf {
    let mut path = prefix.as_os_str().to_owned();
    path.push(".");
    path.push(suffix);
    PathBuf::from(path)
}

/// Creates `path`, failing if it exists. `mode` applies on Unix.
fn write_new(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
    #[cfg(not(unix))]
    let _ = mode;
    options.open(path)?.write_all(contents)
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

#[cfg(test)]
mod tests {
    use kippu_core::keys::parse_verifying_key;

    use super::*;

    fn keygen_into(dir: &Path, pem: bool) -> Result<(SigningKey, String), BoxError> {
        let prefix = dir.join(if pem { "root.v2" } else { "root" });
        keygen(&KeygenArgs {
            pem,
            out: Some(prefix.clone()),
        })?;
        let private = std::fs::read_to_string(with_suffix(&prefix, "key"))?;
        let public = std::fs::read_to_string(with_suffix(&prefix, "pub"))?;
        Ok((
            parse_signing_key("key", &SecretString::from(private))?,
            public,
        ))
    }

    #[test]
    fn keygen_writes_a_matching_pair_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        for pem in [false, true] {
            let (private, public) = keygen_into(dir.path(), pem).unwrap();
            let public = parse_verifying_key("pub", &public).unwrap();
            assert_eq!(private.verifying_key(), public);
            assert!(keygen_into(dir.path(), pem).is_err(), "must not overwrite");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("root.v2.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
