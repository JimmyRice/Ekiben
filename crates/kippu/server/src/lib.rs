#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

pub mod adapters;
pub mod cli;
mod commands;
pub mod config;
mod telemetry;
mod verify;

use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use kippu_core::{App, Config, Kippu, Module, SystemClock};
use kippu_store::BoxError;
use secrecy::ExposeSecret;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::cli::{Cli, Command, ConfigCommand};

/// Builds and runs a Kippu instance from the command line.
///
/// ```no_run
/// fn main() -> std::process::ExitCode {
///     kippu_server::Launcher::new().modules(kippu_core::default_modules()).run()
/// }
/// ```
#[derive(Default)]
pub struct Launcher {
    modules: Vec<Arc<dyn Module>>,
}

impl Launcher {
    /// A launcher with no modules.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds modules, e.g. [`kippu_core::default_modules`].
    #[must_use]
    pub fn modules(mut self, modules: impl IntoIterator<Item = Arc<dyn Module>>) -> Self {
        self.modules.extend(modules);
        self
    }

    /// Adds one module of your own.
    #[must_use]
    pub fn module(mut self, module: impl Module) -> Self {
        self.modules.push(Arc::new(module));
        self
    }

    /// Parses the command line, runs the chosen command and returns the exit code.
    pub fn run(self) -> ExitCode {
        let cli = Cli::parse();
        telemetry::init(cli.log_format);
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => return fail(&error.into()),
        };
        let result = runtime.block_on(async {
            match &cli.command {
                Command::Serve(args) => commands::serve(&cli, args, self.modules).await,
                Command::Worker(args) => commands::worker(&cli, args, self.modules).await,
                Command::Migrate(args) => commands::migrate(&cli, args).await,
                Command::Keygen(args) => commands::keygen(args),
                Command::RootToken(args) => commands::root_token(&cli, args),
                Command::Verify(args) => verify::run(args),
                Command::Config(ConfigCommand::Check(args)) => commands::config_check(&cli, args),
            }
        });
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail(&error),
        }
    }
}

fn fail(error: &BoxError) -> ExitCode {
    tracing::error!(%error, "kippu failed");
    eprintln!("error: {error}");
    ExitCode::FAILURE
}

/// Connects to the configured database, migrates it if configured to, and builds the app.
pub async fn assemble(config: Config, modules: Vec<Arc<dyn Module>>) -> Result<App, BoxError> {
    let store = adapters::connect(config.database.url.expose_secret()).await?;
    if config.database.auto_migrate {
        store.migrate().await?;
    }
    tracing::info!(backend = store.capabilities().backend, "database ready");
    Ok(Kippu::new()
        .modules(modules)
        .build(config, store, Arc::new(SystemClock))?)
}

/// Serves `app` on `listener` until `shutdown` is cancelled, running background tasks too if
/// `workers` is set. In-flight requests and tasks finish before this returns.
pub async fn serve_app(
    app: &App,
    listener: TcpListener,
    shutdown: CancellationToken,
    workers: bool,
) -> std::io::Result<()> {
    let tracker = TaskTracker::new();
    if workers {
        app.spawn_tasks(&tracker, &shutdown);
    }
    tracker.close();
    axum::serve(listener, app.router())
        .with_graceful_shutdown(shutdown.clone().cancelled_owned())
        .await?;
    shutdown.cancel();
    tracker.wait().await;
    Ok(())
}
