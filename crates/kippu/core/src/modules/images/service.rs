//! Image use cases: what may be uploaded, who sees what, and keeping the object store and the
//! database in step. Independent of HTTP.

use bytes::Bytes;
use kippu_domain::image::{EventImage, ImageFormat};
use kippu_domain::{EventId, ImageId, ValidationError};
use kippu_store::{ObjectError, StoredObject};

use super::storage::ImageStorage;
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult, ProblemKind, StatusCode};
use crate::modules::catalog::service::{visible_event, writable_event};

/// How long clients and CDNs may keep an image: forever, since an image id never gets other
/// bytes.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// An image and the URL clients load it from.
#[derive(Debug, Clone)]
pub struct PublishedImage {
    /// The image.
    pub image: EventImage,
    /// A CDN or bucket URL if the deployment serves images publicly, otherwise this API.
    pub url: String,
}

/// Where an image's bytes are to be had.
#[derive(Debug)]
pub enum ImageContent {
    /// At this URL (`images.public_base_url`).
    Elsewhere(String),
    /// Here, streamed from the object store.
    Stored {
        /// Its format.
        format: ImageFormat,
        /// Whether shared caches may keep it: not for an unpublished event's images.
        public: bool,
        /// The object.
        object: StoredObject,
    },
}

/// The deployment's image storage, or 501 if it has none.
pub fn storage(state: &AppState) -> ApiResult<&ImageStorage> {
    state.images().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            ProblemKind::IMAGES_NOT_CONFIGURED,
            "this deployment has no object storage for images (images.url)",
        )
    })
}

fn published(storage: &ImageStorage, image: EventImage) -> PublishedImage {
    let url = match &storage.public_base_url {
        Some(base) => format!("{base}/{}", image.object_key()),
        None => format!("/v1/events/{}/images/{}", image.event_id, image.id),
    };
    PublishedImage { image, url }
}

fn storage_failed(error: ObjectError) -> ApiError {
    ApiError::unavailable(error)
}

/// An image of `event_id`, or 404 — also for an image of another event.
async fn image_of(state: &AppState, event_id: EventId, image_id: ImageId) -> ApiResult<EventImage> {
    state
        .store()
        .event_image(image_id)
        .await?
        .filter(|image| image.event_id == event_id)
        .ok_or_else(|| ApiError::not_found("image"))
}

/// Adds an image to an event the caller may edit.
///
/// `declared` is the format the uploader claims (`None` if it named none we accept), and
/// `body` reads the bytes. It is awaited only once the caller is known to be allowed, so
/// strangers cannot make an instance buffer uploads; the bytes must really be an image of the
/// declared format.
#[tracing::instrument(skip_all)]
pub async fn upload_image(
    state: &AppState,
    principal: &Principal,
    event_id: EventId,
    declared: Option<ImageFormat>,
    body: impl Future<Output = ApiResult<Bytes>> + Send,
) -> ApiResult<PublishedImage> {
    let storage = storage(state)?;
    let event = writable_event(state, principal, event_id).await?;
    let unsupported = |detail: &'static str| {
        ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ProblemKind::UNSUPPORTED_MEDIA_TYPE,
            detail,
        )
    };
    let declared = declared
        .ok_or_else(|| unsupported("Content-Type must be image/png, jpeg, webp or avif"))?;
    let bytes = body.await?;
    if ImageFormat::sniff(&bytes) != Some(declared) {
        return Err(unsupported(
            "the body is not an image of the declared Content-Type",
        ));
    }

    let existing = state.store().event_images(event.id).await?;
    let limit = state.config().images.max_per_event;
    if existing.len() >= limit as usize {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ProblemKind::TOO_MANY_IMAGES,
            format!("an event has at most {limit} images"),
        ));
    }
    let image = EventImage {
        id: ImageId::generate(),
        event_id: event.id,
        format: declared,
        size_bytes: bytes.len() as u64,
        position: existing
            .iter()
            .map(|image| image.position)
            .max()
            .map_or(0, |last| last.saturating_add(1)),
        created_at: state.now(),
    };

    // Bytes first: a recorded image always has its object; a failed insert leaves an orphan
    // object at worst, which is removed right away when possible.
    kippu_telemetry::call(
        "object storage",
        "put",
        storage
            .objects
            .put(&image.object_key(), bytes, declared.media_type(), IMMUTABLE),
    )
    .await
    .map_err(storage_failed)?;
    if let Err(error) = state.store().insert_event_image(&image).await {
        if let Err(cleanup) = kippu_telemetry::call(
            "object storage",
            "delete",
            storage.objects.delete(&image.object_key()),
        )
        .await
        {
            tracing::warn!(image = %image.id, %cleanup, "could not remove an orphaned image");
        }
        return Err(error.into());
    }
    tracing::info!(event = %event.id, image = %image.id, size = image.size_bytes, "image uploaded");
    Ok(published(storage, image))
}

/// The images of an event the caller may see, in gallery order.
#[tracing::instrument(skip_all)]
pub async fn event_images(
    state: &AppState,
    principal: Option<&Principal>,
    event_id: EventId,
) -> ApiResult<Vec<PublishedImage>> {
    let storage = storage(state)?;
    let event = visible_event(state, principal, event_id).await?;
    let images = state.store().event_images(event.id).await?;
    Ok(images
        .into_iter()
        .map(|image| published(storage, image))
        .collect())
}

/// An image of an event the caller may see: where it is served, or its bytes.
#[tracing::instrument(skip_all)]
pub async fn image_content(
    state: &AppState,
    principal: Option<&Principal>,
    event_id: EventId,
    image_id: ImageId,
) -> ApiResult<ImageContent> {
    let storage = storage(state)?;
    let event = visible_event(state, principal, event_id).await?;
    let image = image_of(state, event.id, image_id).await?;
    if let Some(base) = &storage.public_base_url {
        return Ok(ImageContent::Elsewhere(format!(
            "{base}/{}",
            image.object_key()
        )));
    }
    let object = kippu_telemetry::call(
        "object storage",
        "get",
        storage.objects.get(&image.object_key()),
    )
    .await
    .map_err(|error| match error {
        ObjectError::NotFound => ApiError::not_found("image"),
        other @ ObjectError::Failed(_) => storage_failed(other),
    })?;
    Ok(ImageContent::Stored {
        format: image.format,
        public: event.is_public(),
        object,
    })
}

/// Deletes an image of an event the caller may edit.
#[tracing::instrument(skip_all)]
pub async fn delete_image(
    state: &AppState,
    principal: &Principal,
    event_id: EventId,
    image_id: ImageId,
) -> ApiResult<()> {
    let storage = storage(state)?;
    let event = writable_event(state, principal, event_id).await?;
    let image = image_of(state, event.id, image_id).await?;
    // The record first, so the image disappears from the API even if the object lingers.
    state.store().delete_event_image(image.id).await?;
    if let Err(error) = kippu_telemetry::call(
        "object storage",
        "delete",
        storage.objects.delete(&image.object_key()),
    )
    .await
    {
        tracing::warn!(image = %image.id, %error, "image forgotten, but its object remains");
    }
    Ok(())
}

/// Puts an event's images in a new order: `order` lists every image exactly once.
#[tracing::instrument(skip_all)]
pub async fn order_images(
    state: &AppState,
    principal: &Principal,
    event_id: EventId,
    order: &[ImageId],
) -> ApiResult<Vec<PublishedImage>> {
    let storage = storage(state)?;
    let event = writable_event(state, principal, event_id).await?;
    let current = state.store().event_images(event.id).await?;
    let mut given = order.to_vec();
    let mut expected: Vec<ImageId> = current.iter().map(|image| image.id).collect();
    given.sort_unstable();
    expected.sort_unstable();
    if given != expected {
        return Err(ValidationError::new(
            "image_ids",
            "must list every image of the event exactly once",
        )
        .into());
    }
    state.store().set_image_positions(event.id, order).await?;
    let images = state.store().event_images(event.id).await?;
    Ok(images
        .into_iter()
        .map(|image| published(storage, image))
        .collect())
}
