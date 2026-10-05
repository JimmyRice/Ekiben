//! Assembling an instance: [`Kippu`] builds an [`App`] from modules, configuration, a store
//! and a clock.

use std::fmt::Display;
use std::sync::Arc;

use axum::Router;
use kippu_domain::Timestamp;
use kippu_store::{AuditEntry, Store};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::auth::tokens::Tokens;
use crate::auth::{Permission, Policy, Principal, Scope};
use crate::clock::Clock;
use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::keys::{KeyError, TicketKeys, parse_signing_key, parse_verifying_key};
use crate::module::{BackgroundTask, Module};
use crate::{http, workers};

/// Shared, read-only state every handler and task receives. Cheap to clone.
///
/// Holds no request or session state: everything mutable lives in the store, which is what
/// makes Kippu stateless and horizontally scalable.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
    tokens: Tokens,
    tickets: TicketKeys,
    policy: Policy,
}

impl AppState {
    /// The configuration the instance was started with.
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// The database.
    pub fn store(&self) -> &dyn Store {
        self.inner.store.as_ref()
    }

    /// The current time.
    pub fn now(&self) -> Timestamp {
        self.inner.clock.now()
    }

    /// Issues and verifies bearer tokens.
    pub fn tokens(&self) -> &Tokens {
        &self.inner.tokens
    }

    /// Signs tickets and lists the keys verifiers should trust.
    pub fn tickets(&self) -> &TicketKeys {
        &self.inner.tickets
    }

    /// Which roles hold which permissions.
    pub fn policy(&self) -> &Policy {
        &self.inner.policy
    }

    /// Fails with 403 unless `principal` may perform `permission` within `scope`.
    pub fn authorize(
        &self,
        principal: &Principal,
        permission: Permission,
        scope: Scope,
    ) -> ApiResult<()> {
        if self.policy().permits(principal, permission, scope) {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }

    /// Records a privileged action in the audit log.
    pub async fn audit(
        &self,
        principal: &Principal,
        action: &str,
        target: impl Display,
    ) -> ApiResult<()> {
        let entry = AuditEntry {
            at: self.now(),
            actor: principal.actor(),
            action: action.to_owned(),
            target: target.to_string(),
        };
        Ok(self.store().append_audit(&entry).await?)
    }
}

/// Builder for a Kippu instance.
#[derive(Default)]
pub struct Kippu {
    modules: Vec<Arc<dyn Module>>,
}

impl Kippu {
    /// A builder with no modules.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one module.
    #[must_use]
    pub fn module(mut self, module: impl Module) -> Self {
        self.modules.push(Arc::new(module));
        self
    }

    /// Adds several modules, e.g. [`default_modules`](crate::default_modules).
    #[must_use]
    pub fn modules(mut self, modules: impl IntoIterator<Item = Arc<dyn Module>>) -> Self {
        self.modules.extend(modules);
        self
    }

    /// Builds the instance. Fails if a configured key cannot be parsed.
    pub fn build(
        self,
        config: Config,
        store: Arc<dyn Store>,
        clock: Arc<dyn Clock>,
    ) -> Result<App, KeyError> {
        let mut policy = Policy::default();
        for module in &self.modules {
            for (role, permission) in module.grants() {
                policy.grant(role, permission);
            }
        }

        let ticket_key =
            parse_signing_key("keys.ticket_signing_key", &config.keys.ticket_signing_key)?;
        let retired = config
            .keys
            .retired_ticket_keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                parse_verifying_key(&format!("keys.retired_ticket_keys[{index}]"), key)
            })
            .collect::<Result<_, _>>()?;
        let tasks = self
            .modules
            .iter()
            .flat_map(|module| module.tasks(&config))
            .collect();

        let state = AppState {
            inner: Arc::new(Inner {
                tokens: Tokens::from_config(&config)?,
                tickets: TicketKeys::new(&ticket_key, retired),
                config,
                store,
                clock,
                policy,
            }),
        };
        let router = http::router(&state, &self.modules);
        for module in &self.modules {
            tracing::debug!(module = module.name(), "module loaded");
        }
        Ok(App {
            state,
            router,
            tasks,
        })
    }
}

/// A built instance: an HTTP router plus background tasks, sharing one [`AppState`].
pub struct App {
    state: AppState,
    router: Router,
    tasks: Vec<BackgroundTask>,
}

impl App {
    /// The shared state.
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// The HTTP application, ready to be served.
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// The background tasks of every module.
    pub fn tasks(&self) -> &[BackgroundTask] {
        &self.tasks
    }

    /// Looks a background task up by name.
    pub fn task(&self, name: &str) -> Option<&BackgroundTask> {
        self.tasks.iter().find(|task| task.name() == name)
    }

    /// Starts every background task on `tracker`; they stop when `shutdown` is cancelled.
    pub fn spawn_tasks(&self, tracker: &TaskTracker, shutdown: &CancellationToken) {
        for task in &self.tasks {
            tracker.spawn(workers::run(
                task.clone(),
                self.state.clone(),
                shutdown.clone(),
            ));
        }
    }
}
