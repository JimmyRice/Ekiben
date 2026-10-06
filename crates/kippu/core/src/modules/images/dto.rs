use kippu_domain::ImageId;
use kippu_domain::image::EventImage;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::service::PublishedImage;

/// An event image and where to get it.
#[derive(Debug, Serialize, ToSchema)]
pub struct EventImageView {
    /// The image.
    #[serde(flatten)]
    pub image: EventImage,
    /// Where clients load it from: a CDN or bucket URL if the deployment serves images
    /// publicly, otherwise this API.
    pub url: String,
}

/// A new order for an event's images.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageOrder {
    /// Every image of the event, exactly once, first to last.
    pub image_ids: Vec<ImageId>,
}

impl From<PublishedImage> for EventImageView {
    fn from(published: PublishedImage) -> Self {
        Self {
            image: published.image,
            url: published.url,
        }
    }
}
