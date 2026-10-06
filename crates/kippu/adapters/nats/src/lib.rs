#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

use std::time::Duration;

use async_nats::HeaderMap;
use async_nats::jetstream::consumer::pull::Config as PullConfig;
use async_nats::jetstream::consumer::{AckPolicy, PullConsumer};
use async_nats::jetstream::stream::{Config as StreamConfig, RetentionPolicy, Stream};
use async_nats::jetstream::{self, Context, Message};
use async_trait::async_trait;
use futures_util::StreamExt;
use kippu_domain::purchase::PurchaseRequest;
use kippu_store::{EventBus, InboxDelivery, OutboxRecord, PurchaseInbox, StoreError, StoreResult};

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
/// - The inbox is a work-queue stream: [`PurchaseInbox::enqueue`] publishes a request with
///   `Nats-Msg-Id` set to its id and returns once JetStream acknowledged it, and every
///   instance pulls from one durable consumer, acknowledging each request once persisted.
/// - The bus is a stream of `<prefix>.events.<topic>` subjects, one message per outbox event
///   with `Nats-Msg-Id` `outbox-<sequence>` and a `Kippu-Sequence` header; the last message
///   tells relays where to resume, so the position needs no table of its own.
#[derive(Debug, Clone)]
pub struct NatsQueue {
    jetstream: Context,
    inbox_subject: String,
    events_prefix: String,
    consumer: PullConsumer,
    events: Stream,
    receive_wait: Duration,
}

fn unavailable(error: impl std::error::Error + Send + Sync + 'static) -> StoreError {
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

    async fn publish(
        &self,
        subject: String,
        id: String,
        headers: HeaderMap,
        payload: Vec<u8>,
    ) -> StoreResult<()> {
        let mut headers = headers;
        headers.insert("Nats-Msg-Id", id.as_str());
        self.jetstream
            .publish_with_headers(subject, headers, payload.into())
            .await
            .map_err(unavailable)?
            .await
            .map_err(unavailable)?;
        Ok(())
    }
}

/// A request pulled from the inbox.
struct NatsDelivery {
    request: PurchaseRequest,
    message: Message,
}

#[async_trait]
impl InboxDelivery for NatsDelivery {
    fn request(&self) -> &PurchaseRequest {
        &self.request
    }

    async fn ack(self: Box<Self>) -> StoreResult<()> {
        // A double ack waits for the server to confirm, so a crash right after cannot
        // leave the message to be delivered again silently.
        self.message
            .double_ack()
            .await
            .map_err(|error| StoreError::Unavailable(error.to_string().into()))
    }
}

#[async_trait]
impl PurchaseInbox for NatsQueue {
    async fn enqueue(&self, request: &PurchaseRequest) -> StoreResult<()> {
        let payload = serde_json::to_vec(request).map_err(StoreError::backend)?;
        self.publish(
            self.inbox_subject.clone(),
            request.id.to_string(),
            HeaderMap::new(),
            payload,
        )
        .await
    }

    async fn receive(&self, limit: usize) -> StoreResult<Vec<Box<dyn InboxDelivery>>> {
        let mut batch = self
            .consumer
            .fetch()
            .max_messages(limit)
            .expires(self.receive_wait)
            .messages()
            .await
            .map_err(unavailable)?;
        let mut deliveries: Vec<Box<dyn InboxDelivery>> = Vec::new();
        while let Some(message) = batch.next().await {
            let message = message.map_err(StoreError::Unavailable)?;
            match serde_json::from_slice::<PurchaseRequest>(&message.payload) {
                Ok(request) => deliveries.push(Box::new(NatsDelivery { request, message })),
                Err(error) => {
                    // Not something Kippu published: drop it rather than retry it forever.
                    message
                        .ack()
                        .await
                        .map_err(|error| StoreError::Unavailable(error.to_string().into()))?;
                    return Err(StoreError::backend(format!(
                        "unreadable inbox message: {error}"
                    )));
                }
            }
        }
        Ok(deliveries)
    }
}

/// The header carrying an event's outbox sequence.
pub const SEQUENCE_HEADER: &str = "Kippu-Sequence";

#[async_trait]
impl EventBus for NatsQueue {
    async fn last_published(&self) -> StoreResult<i64> {
        let mut events = self.events.clone();
        let last = events
            .info()
            .await
            .map_err(unavailable)?
            .state
            .last_sequence;
        if last == 0 {
            return Ok(0);
        }
        let message = events.get_raw_message(last).await.map_err(unavailable)?;
        message
            .headers
            .get(SEQUENCE_HEADER)
            .and_then(|value| value.as_str().parse().ok())
            .ok_or_else(|| StoreError::backend("the last event on the bus has no Kippu-Sequence"))
    }

    async fn publish(&self, record: &OutboxRecord) -> StoreResult<()> {
        let payload = serde_json::to_vec(&serde_json::json!({
            "sequence": record.sequence,
            "created_at": record.created_at,
            "event": record.event,
        }))
        .map_err(StoreError::backend)?;
        let mut headers = HeaderMap::new();
        headers.insert(SEQUENCE_HEADER, record.sequence.to_string().as_str());
        NatsQueue::publish(
            self,
            format!("{}.{}", self.events_prefix, record.event.topic()),
            format!("outbox-{}", record.sequence),
            headers,
            payload,
        )
        .await
    }
}
