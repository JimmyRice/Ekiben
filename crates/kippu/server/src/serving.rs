//! Putting an instance together from its configuration, and serving it.

use std::sync::Arc;

use kippu_core::{App, Config, Kippu, Module, SystemClock};
use kippu_store::BoxError;
use secrecy::ExposeSecret;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::adapters;

/// Connects to the configured database, migrates it if configured to, and builds the app.
pub async fn assemble(config: Config, modules: Vec<Arc<dyn Module>>) -> Result<App, BoxError> {
    let store = adapters::connect(config.database.url.expose_secret()).await?;
    if config.database.auto_migrate {
        store.migrate().await?;
    }
    tracing::info!(backend = store.capabilities().backend, "database ready");
    let mut kippu = Kippu::new().modules(modules);
    if let Some(url) = &config.queue.url {
        let queue = adapters::connect_queue(url.expose_secret()).await?;
        tracing::info!(
            inbox = config.queue.purchase_inbox,
            relay = config.queue.relay_events,
            "queue ready"
        );
        if config.queue.purchase_inbox {
            kippu = kippu.inbox(queue.inbox);
        }
        if config.queue.relay_events {
            kippu = kippu.event_bus(queue.bus);
        }
    }
    if let Some(url) = &config.images.url {
        let options: Vec<(String, String)> = config
            .images
            .options
            .iter()
            .map(|(name, value)| (name.clone(), value.expose_secret().to_owned()))
            .collect();
        kippu = kippu.object_storage(adapters::connect_objects(url, &options)?);
    }
    Ok(kippu.build(config, store, Arc::new(SystemClock))?)
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
    let service = app
        .router()
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, service)
        .with_graceful_shutdown(shutdown.clone().cancelled_owned())
        .await?;
    shutdown.cancel();
    tracker.wait().await;
    Ok(())
}
