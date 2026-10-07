use async_trait::async_trait;
use kippu_domain::image::EventImage;
use kippu_domain::{EventId, ImageId};

use crate::StoreResult;

/// What is known about event images; the bytes are in object storage.
#[async_trait]
pub trait ImageStore {
    /// Records an uploaded image.
    async fn insert_event_image(&self, image: &EventImage) -> StoreResult<()>;

    /// Looks an image up by id.
    async fn event_image(&self, id: ImageId) -> StoreResult<Option<EventImage>>;

    /// An event's images by position, then id.
    async fn event_images(&self, event: EventId) -> StoreResult<Vec<EventImage>>;

    /// Gives each listed image of `event` its index as position, in one transaction. Images
    /// of other events are left alone.
    async fn set_image_positions(&self, event: EventId, order: &[ImageId]) -> StoreResult<()>;

    /// Forgets an image. Returns whether it existed.
    async fn delete_event_image(&self, id: ImageId) -> StoreResult<bool>;
}
