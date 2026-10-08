//! Assembling an instance: [`Kippu`] builds an [`App`] from modules, configuration, a store
//! and a clock.

use std::fmt::Display;
use std::sync::Arc;

use axum::Router;
use ed25519_dalek::SigningKey;
use kippu_domain::Timestamp;
use kippu_store::{AuditEntry, EventBus, ObjectStorage, PurchaseInbox, Store};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::auth::tokens::Tokens;
use crate::auth::{Permission, Policy, Principal, Scope};
use crate::clock::Clock;
use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::keys::{ConfigError, TicketKeys, parse_signing_key, parse_verifying_key};
use crate::module::{BackgroundTask, Module};
use crate::modules::images::ImageStorage;
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
    webhook_key: Option<SigningKey>,
    images: Option<ImageStorage>,
    inbox: Option<Arc<dyn PurchaseInbox>>,
    event_bus: Option<Arc<dyn EventBus>>,
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

    /// Signs webhook deliveries, if the deployment configured a key.
    pub fn webhook_key(&self) -> Option<&SigningKey> {
        self.inner.webhook_key.as_ref()
    }

    /// Where event images are kept, if the deployment has an object store.
    pub fn images(&self) -> Option<&ImageStorage> {
        self.inner.images.as_ref()
    }

    /// The queue purchase requests pass through before the database, if any.
    pub fn inbox(&self) -> Option<&dyn PurchaseInbox> {
        self.inner.inbox.as_deref()
    }

    /// Where integration events are published, if anywhere.
    pub fn event_bus(&self) -> Option<&dyn EventBus> {
        self.inner.event_bus.as_deref()
    }

    /// Which roles hold which permissions.
    pub fn policy(&self) -> &Policy {
        &self.inner.policy
    }

    /// Fails with 403 unless `principal` may perform `permission` within `scope`.
    #[track_caller]
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
    objects: Option<Arc<dyn ObjectStorage>>,
    inbox: Option<Arc<dyn PurchaseInbox>>,
    event_bus: Option<Arc<dyn EventBus>>,
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

    /// Keeps event images in `objects` (see [`kippu_store::ObjectStorage`]). Deployments
    /// connect the adapter `images.url` names; tests pass an in-memory one. Without it, image
    /// uploads are unavailable.
    #[must_use]
    pub fn object_storage(mut self, objects: Arc<dyn ObjectStorage>) -> Self {
        self.objects = Some(objects);
        self
    }

    /// Passes purchase requests through `inbox` before the database (see
    /// [`kippu_store::PurchaseInbox`]). Adds the `purchase-inbox` task that persists them.
    #[must_use]
    pub fn inbox(mut self, inbox: Arc<dyn PurchaseInbox>) -> Self {
        self.inbox = Some(inbox);
        self
    }

    /// Publishes integration events from the outbox to `bus`. Adds the `event-relay` task.
    #[must_use]
    pub fn event_bus(mut self, bus: Arc<dyn EventBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Builds the instance. Fails if a configuration value (such as a key) is unusable.
    pub fn build(
        self,
        config: Config,
        store: Arc<dyn Store>,
        clock: Arc<dyn Clock>,
    ) -> Result<App, ConfigError> {
        let mut policy = Policy::default();
        for module in &self.modules {
            for (role, permission) in module.grants() {
                policy.grant(role, permission);
            }
        }

        config.webhooks.check()?;
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
        let webhook_key = config
            .keys
            .webhook_signing_key
            .as_ref()
            .map(|key| parse_signing_key("keys.webhook_signing_key", key))
            .transpose()?;
        if config.images.url.is_some() && self.objects.is_none() {
            return Err(ConfigError {
                name: "images.url".to_owned(),
                reason: "is set, but no object storage was given to `Kippu::object_storage`",
            });
        }
        let images = self.objects.map(|objects| ImageStorage {
            objects,
            public_base_url: config
                .images
                .public_base_url
                .as_ref()
                .map(|base| base.trim_end_matches('/').to_owned()),
        });
        let mut tasks: Vec<BackgroundTask> = self
            .modules
            .iter()
            .flat_map(|module| module.tasks(&config))
            .collect();
        tasks.extend(crate::messaging::tasks(
            &config,
            self.inbox.is_some(),
            self.event_bus.is_some(),
        ));

        let state = AppState {
            inner: Arc::new(Inner {
                tokens: Tokens::from_config(&config)?,
                tickets: TicketKeys::new(&ticket_key, retired),
                webhook_key,
                images,
                inbox: self.inbox,
                event_bus: self.event_bus,
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
