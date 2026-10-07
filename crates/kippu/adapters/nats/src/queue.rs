//! Connecting to JetStream: the streams and the inbox consumer Kippu uses.

use std::time::Duration;

use async_nats::HeaderMap;
use async_nats::jetstream::consumer::pull::Config as PullConfig;
use async_nats::jetstream::consumer::{AckPolicy, PullConsumer};
use async_nats::jetstream::stream::{Config as StreamConfig, RetentionPolicy, Stream};
use async_nats::jetstream::{self, Context};
use kippu_store::{StoreError, StoreResult};

/// Names of the streams and subjects Kippu uses; the defaults suit one deployment per NATS
/// account. Change `prefix` to run several deployments on one.
#[derive(Debug, Clone)]
pub struct NatsOptions {
    /// Prefix of stream names (`<PREFIX>_PURCHASES`, `<PREFIX>_EVENTS`) and, lower-cased, of
    /// subjects (`<prefix>.purchases`, `<prefix>.events.<topic>`). Default `KIPPU`.
    pub prefix: String,
    /// How long JetStream remembers message ids to drop duplicates. Retries and concurrent
    /// relays within it are harmless. Default ten minutes.
    pub duplicate_window: Duration,
    /// How long a worker may hold an inbox message before it is redelivered. Default 30 s.
    pub ack_wait: Duration,
    /// How long `receive` waits for messages when the inbox is empty. Default 100 ms.
    pub receive_wait: Duration,
}

impl Default for NatsOptions {
    fn default() -> Self {
        Self {
            prefix: "KIPPU".to_owned(),
            duplicate_window: Duration::from_secs(600),
            ack_wait: Duration::from_secs(30),
            receive_wait: Duration::from_millis(100),
        }
    }
}

/// JetStream as Kippu's purchase inbox and event bus.
///
/// - The inbox is a work-queue stream: [`PurchaseInbox::enqueue`](kippu_store::PurchaseInbox::enqueue) publishes a request with
///   `Nats-Msg-Id` set to its id and returns once JetStream acknowledged it, and every
///   instance pulls from one durable consumer, acknowledging each request once persisted.
/// - The bus is a stream of `<prefix>.events.<topic>` subjects, one message per outbox event
///   with `Nats-Msg-Id` `outbox-<sequence>` and a `Kippu-Sequence` header; the last message
///   tells relays where to resume, so the position needs no table of its own.
#[derive(Debug, Clone)]
pub struct NatsQueue {
    jetstream: Context,
    pub(crate) inbox_subject: String,
    pub(crate) events_prefix: String,
    pub(crate) consumer: PullConsumer,
    pub(crate) events: Stream,
    pub(crate) receive_wait: Duration,
}

pub(crate) fn unavailable(error: impl std::error::Error + Send + Sync + 'static) -> StoreError {
    StoreError::Unavailable(Box::new(error))
}

impl NatsQueue {
    /// Connects to `url` (e.g. `nats://nats.internal:4222`) and creates the streams and the
    /// inbox consumer if they do not exist.
    pub async fn connect(url: &str, options: NatsOptions) -> StoreResult<Self> {
        let client = async_nats::connect(url).await.map_err(unavailable)?;
        let jetstream = jetstream::new(client);
        let subject_prefix = options.prefix.to_lowercase();
        let inbox_subject = format!("{subject_prefix}.purchases");
        let purchases = jetstream
            .get_or_create_stream(StreamConfig {
                name: format!("{}_PURCHASES", options.prefix),
                subjects: vec![inbox_subject.clone()],
                retention: RetentionPolicy::WorkQueue,
                duplicate_window: options.duplicate_window,
                ..StreamConfig::default()
            })
            .await
            .map_err(unavailable)?;
        let consumer = purchases
            .get_or_create_consumer(
                "kippu-inbox",
                PullConfig {
                    durable_name: Some("kippu-inbox".to_owned()),
                    ack_policy: AckPolicy::Explicit,
                    ack_wait: options.ack_wait,
                    ..PullConfig::default()
                },
            )
            .await
            .map_err(unavailable)?;
        let events_prefix = format!("{subject_prefix}.events");
        let events = jetstream
            .get_or_create_stream(StreamConfig {
                name: format!("{}_EVENTS", options.prefix),
                subjects: vec![format!("{events_prefix}.>")],
                duplicate_window: options.duplicate_window,
                ..StreamConfig::default()
            })
            .await
            .map_err(unavailable)?;
        Ok(Self {
            jetstream,
            inbox_subject,
            events_prefix,
            consumer,
            events,
            receive_wait: options.receive_wait,
        })
    }

    /// Publishes with `id` as `Nats-Msg-Id`, waiting for JetStream to acknowledge it.
    pub(crate) async fn publish(
        &self,
        subject: String,
        id: String,
        headers: HeaderMap,
        payload: Vec<u8>,
    ) -> StoreResult<()> {
        let mut headers = headers;
        headers.insert("Nats-Msg-Id", id.as_str());
        kippu_telemetry::call("nats", "publish", async {
            self.jetstream
                .publish_with_headers(subject, headers, payload.into())
                .await
                .map_err(unavailable)?
                .await
                .map_err(unavailable)?;
            Ok(())
        })
        .await
    }
}
