//! The module system: how features plug into Kippu.
//!
//! Every built-in feature (accounts, catalog, purchasing, …) is a [`Module`], and so is
//! anything you add. A module contributes routes, permissions and background tasks:
//!
//! ```ignore
//! pub struct Community;
//!
//! impl Module for Community {
//!     fn name(&self) -> &'static str { "community" }
//!     fn grants(&self) -> Vec<(Role, Permission)> { vec![(Role::User, POSTS_WRITE)] }
//!     fn routes(&self) -> OpenApiRouter<AppState> { OpenApiRouter::new().routes(routes!(create_post)) }
//! }
//!
//! Kippu::new().modules(kippu_core::default_modules()).module(Community)
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use kippu_domain::account::Role;
use kippu_store::BoxError;
use utoipa_axum::router::OpenApiRouter;

use crate::app::AppState;
use crate::auth::Permission;
use crate::config::Config;

/// A feature of a Kippu instance.
pub trait Module: Send + Sync + 'static {
    /// A short, unique name, used in logs.
    fn name(&self) -> &'static str;

    /// Permissions this module checks, and the lowest role that holds each.
    fn grants(&self) -> Vec<(Role, Permission)> {
        Vec::new()
    }

    /// HTTP routes, with their OpenAPI documentation. Paths are absolute (`/v1/...`).
    fn routes(&self) -> OpenApiRouter<AppState>;

    /// Routes that accept bodies larger than `server.max_body_bytes`, such as uploads.
    fn body_limits(&self, config: &Config) -> Vec<BodyLimit> {
        let _ = config;
        Vec::new()
    }

    /// Periodic background work.
    fn tasks(&self, config: &Config) -> Vec<BackgroundTask> {
        let _ = config;
        Vec::new()
    }
}

/// A larger request body limit for the routes matching `path`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLimit {
    /// The route's path as declared, e.g. `/v1/events/{event_id}/images`; `{…}` segments
    /// match any one segment.
    pub path: &'static str,
    /// The largest body accepted, in bytes.
    pub max_bytes: usize,
}

impl BodyLimit {
    /// Whether a request path falls under this limit.
    pub fn matches(&self, path: &str) -> bool {
        let mut pattern = self.path.split('/');
        let mut segments = path.split('/');
        loop {
            match (pattern.next(), segments.next()) {
                (None, None) => return true,
                (Some(expected), Some(actual)) => {
                    let wildcard = expected.starts_with('{') && expected.ends_with('}');
                    if !wildcard && expected != actual {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }
}

/// What one run of a background task accomplished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Nothing (more) to do: wait for the interval before running again.
    Idle,
    /// A full batch was processed and more may be waiting: run again right away.
    MoreWork,
}

type TaskFn = dyn Fn(AppState) -> Pin<Box<dyn Future<Output = Result<Progress, BoxError>> + Send>>
    + Send
    + Sync;

/// Work that runs periodically on every instance with workers enabled.
///
/// Tasks must be safe to run concurrently on many instances: they claim work through the
/// store's leases and conditional updates, never through local state.
#[derive(Clone)]
pub struct BackgroundTask {
    name: &'static str,
    interval: Duration,
    run: Arc<TaskFn>,
}

impl BackgroundTask {
    /// A task that runs `run`, then waits `interval` whenever it reports [`Progress::Idle`].
    pub fn every<F, Fut>(name: &'static str, interval: Duration, run: F) -> Self
    where
        F: Fn(AppState) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Progress, BoxError>> + Send + 'static,
    {
        Self {
            name,
            interval,
            run: Arc::new(move |state| Box::pin(run(state))),
        }
    }

    /// The task's name.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// How long the task rests when idle.
    pub const fn interval(&self) -> Duration {
        self.interval
    }

    /// Runs the task once. Tests use this to drive workers deterministically.
    pub async fn run_once(&self, state: AppState) -> Result<Progress, BoxError> {
        (self.run)(state).await
    }
}

impl std::fmt::Debug for BackgroundTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundTask")
            .field("name", &self.name)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}
