//! The purchase inbox: a work-queue stream every instance pulls from.

use async_nats::HeaderMap;
use async_nats::jetstream::Message;
use async_trait::async_trait;
use futures_util::StreamExt;
use kippu_domain::purchase::PurchaseRequest;
use kippu_store::{InboxDelivery, PurchaseInbox, StoreError, StoreResult};

use crate::queue::{NatsQueue, unavailable};

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
