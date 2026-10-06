//! Event images, kept in object storage.
//!
//! Organizers upload images for their events; anyone who can see an event can list and load
//! them. The bytes go to the deployment's object store (S3 and compatible services, GCS or
//! Azure) — never to an instance's disk, since instances are stateless — and the database
//! keeps the gallery order. Without an object store the endpoints answer
//! `501 images-not-configured`. With `images.public_base_url` (a CDN in front of the bucket)
//! clients are sent there instead of loading images through Kippu.

mod dto;
mod routes;
pub(crate) mod storage;

use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::config::Config;
use crate::module::{BodyLimit, Module};

pub use dto::*;
pub use storage::ImageStorage;

/// The images module.
#[derive(Debug, Clone, Copy, Default)]
pub struct Images;

impl Module for Images {
    fn name(&self) -> &'static str {
        "images"
    }

    fn routes(&self) -> OpenApiRouter<AppState> {
        OpenApiRouter::new()
            .routes(routes!(routes::upload_image, routes::list_images))
            .routes(routes!(routes::order_images))
            .routes(routes!(routes::get_image, routes::delete_image))
    }

    fn body_limits(&self, config: &Config) -> Vec<BodyLimit> {
        vec![BodyLimit {
            path: "/v1/events/{event_id}/images",
            max_bytes: config.images.max_bytes,
        }]
    }
}
