//! The `kippu` command line: parse it, run the chosen command, report failure.

use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use kippu_core::Module;
use kippu_store::BoxError;

use crate::cli::{Cli, Command, ConfigCommand, LogFormat};
use crate::{commands, verify};

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
        kippu_telemetry::layer::init(cli.log_format.into());
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => return fail(&error.into()),
        };
        let format = cli.log_format;
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
            Err(error) => {
                if format == LogFormat::Json {
                    tracing::error!(%error, "kippu failed");
                }
                fail(&error)
            }
        }
    }
}

fn fail(error: &BoxError) -> ExitCode {
    eprintln!("error: {error}");
    ExitCode::FAILURE
}
