use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, LOCATION};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use kippu_domain::image::{EventImage, ImageFormat};
use kippu_domain::{EventId, ImageId};
use object_store::path::Path as ObjectPath;
use object_store::{
    Attribute, Attributes, GetOptions, ObjectStore, ObjectStoreExt, PutOptions, PutPayload,
};

use super::dto::{EventImageView, ImageOrder};
use super::storage::ImageStorage;
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult, Problem};
use crate::http::Json;
use crate::modules::catalog::service::{visible_event, writable_event};

const TAG: &str = "images";

/// How long clients and CDNs may keep an image: forever, since an image id never gets other
/// bytes.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

fn storage(state: &AppState) -> ApiResult<&ImageStorage> {
    state.images().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "images-not-configured",
            "this deployment has no object storage for images (images.url)",
        )
    })
}

fn object_path(image: &EventImage) -> ObjectPath {
    ObjectPath::from(image.object_key())
}

fn view(storage: &ImageStorage, image: EventImage) -> EventImageView {
    let url = match &storage.public_base_url {
        Some(base) => format!("{base}/{}", image.object_key()),
        None => format!("/v1/events/{}/images/{}", image.event_id, image.id),
    };
    EventImageView { image, url }
}

fn storage_failed(error: object_store::Error) -> ApiError {
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

/// Upload an image: the request body is the image itself.
#[utoipa::path(
    post, path = "/v1/events/{event_id}/images", tag = TAG,
    security(("bearer" = [])),
    params(("event_id" = EventId, Path)),
    request_body(
        description = "The image: PNG, JPEG, WebP or AVIF, at most `images.max_bytes`",
        content(("image/png"), ("image/jpeg"), ("image/webp"), ("image/avif"))
    ),
    responses(
        (status = 201, body = EventImageView),
        (status = 409, description = "The event has the most images allowed", body = Problem),
        (status = 413, body = Problem),
        (status = 415, description = "Not an accepted image format", body = Problem),
        (status = 501, description = "Images are not configured", body = Problem)
    )
)]
pub(crate) async fn upload_image(
    State(state): State<AppState>,
    principal: Principal,
    Path(event_id): Path<EventId>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<EventImageView>)> {
    let storage = storage(&state)?;
    let event = writable_event(&state, &principal, event_id).await?;
    let unsupported = |detail: &'static str| {
        ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported-media-type",
            detail,
        )
    };
    let declared = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<ImageFormat>().ok())
        .ok_or_else(|| unsupported("Content-Type must be image/png, jpeg, webp or avif"))?;
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|error| {
            let mut cause = std::error::Error::source(&error);
            let mut too_large = false;
            while let Some(current) = cause {
                too_large |= current.is::<http_body_util::LengthLimitError>();
                cause = current.source();
            }
            if too_large {
                ApiError::payload_too_large()
            } else {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "invalid-body",
                    "the body could not be read",
                )
            }
        })?;
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
            "too-many-images",
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
    let mut attributes = Attributes::new();
    attributes.insert(Attribute::ContentType, declared.media_type().into());
    attributes.insert(Attribute::CacheControl, IMMUTABLE.into());
    let options = PutOptions {
        attributes,
        ..PutOptions::default()
    };
    storage
        .store
        .put_opts(&object_path(&image), PutPayload::from_bytes(bytes), options)
        .await
        .map_err(storage_failed)?;
    if let Err(error) = state.store().insert_event_image(&image).await {
        if let Err(cleanup) = storage.store.delete(&object_path(&image)).await {
            tracing::warn!(image = %image.id, %cleanup, "could not remove an orphaned image");
        }
        return Err(error.into());
    }
    tracing::info!(event = %event.id, image = %image.id, size = image.size_bytes, "image uploaded");
    Ok((StatusCode::CREATED, Json(view(storage, image))))
}

/// An event's images, in gallery order.
#[utoipa::path(
    get, path = "/v1/events/{event_id}/images", tag = TAG,
    params(("event_id" = EventId, Path)),
    responses((status = 200, body = Vec<EventImageView>), (status = 404, body = Problem), (status = 501, body = Problem))
)]
pub(crate) async fn list_images(
    State(state): State<AppState>,
    principal: Option<Principal>,
    Path(event_id): Path<EventId>,
) -> ApiResult<Json<Vec<EventImageView>>> {
    let storage = storage(&state)?;
    let event = visible_event(&state, principal.as_ref(), event_id).await?;
    let images = state.store().event_images(event.id).await?;
    Ok(Json(
        images
            .into_iter()
            .map(|image| view(storage, image))
            .collect(),
    ))
}

/// The image itself, or a redirect to where the deployment serves it from.
#[utoipa::path(
    get, path = "/v1/events/{event_id}/images/{image_id}", tag = TAG,
    params(("event_id" = EventId, Path), ("image_id" = ImageId, Path)),
    responses(
        (status = 200, description = "The image bytes"),
        (status = 302, description = "Served from `images.public_base_url`"),
        (status = 404, body = Problem)
    )
)]
pub(crate) async fn get_image(
    State(state): State<AppState>,
    principal: Option<Principal>,
    Path((event_id, image_id)): Path<(EventId, ImageId)>,
) -> ApiResult<Response> {
    let storage = storage(&state)?;
    let event = visible_event(&state, principal.as_ref(), event_id).await?;
    let image = image_of(&state, event.id, image_id).await?;
    if let Some(base) = &storage.public_base_url {
        let location = HeaderValue::from_str(&format!("{base}/{}", image.object_key()))
            .map_err(ApiError::internal)?;
        return Ok((StatusCode::FOUND, [(LOCATION, location)]).into_response());
    }
    let object = storage
        .store
        .get_opts(&object_path(&image), GetOptions::default())
        .await
        .map_err(|error| match error {
            object_store::Error::NotFound { .. } => ApiError::not_found("image"),
            other => storage_failed(other),
        })?;
    // Drafts are visible to their organizers only: keep them out of shared caches.
    let cache = if event.is_public() {
        IMMUTABLE
    } else {
        "private, no-store"
    };
    Ok((
        [
            (
                CONTENT_TYPE,
                HeaderValue::from_static(image.format.media_type()),
            ),
            (CACHE_CONTROL, HeaderValue::from_static(cache)),
        ],
        Body::from_stream(object.into_stream()),
    )
        .into_response())
}

/// Delete an image.
#[utoipa::path(
    delete, path = "/v1/events/{event_id}/images/{image_id}", tag = TAG,
    security(("bearer" = [])),
    params(("event_id" = EventId, Path), ("image_id" = ImageId, Path)),
    responses((status = 204), (status = 404, body = Problem))
)]
pub(crate) async fn delete_image(
    State(state): State<AppState>,
    principal: Principal,
    Path((event_id, image_id)): Path<(EventId, ImageId)>,
) -> ApiResult<StatusCode> {
    let storage = storage(&state)?;
    let event = writable_event(&state, &principal, event_id).await?;
    let image = image_of(&state, event.id, image_id).await?;
    // The record first, so the image disappears from the API even if the object lingers.
    state.store().delete_event_image(image.id).await?;
    if let Err(error) = storage.store.delete(&object_path(&image)).await {
        tracing::warn!(image = %image.id, %error, "image forgotten, but its object remains");
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Put an event's images in a new order.
#[utoipa::path(
    put, path = "/v1/events/{event_id}/images/order", tag = TAG,
    security(("bearer" = [])),
    params(("event_id" = EventId, Path)),
    request_body = ImageOrder,
    responses((status = 200, body = Vec<EventImageView>), (status = 422, body = Problem))
)]
pub(crate) async fn order_images(
    State(state): State<AppState>,
    principal: Principal,
    Path(event_id): Path<EventId>,
    Json(order): Json<ImageOrder>,
) -> ApiResult<Json<Vec<EventImageView>>> {
    let storage = storage(&state)?;
    let event = writable_event(&state, &principal, event_id).await?;
    let current = state.store().event_images(event.id).await?;
    let mut given = order.image_ids.clone();
    let mut expected: Vec<ImageId> = current.iter().map(|image| image.id).collect();
    given.sort_unstable();
    expected.sort_unstable();
    if given != expected {
        return Err(kippu_domain::ValidationError::new(
            "image_ids",
            "must list every image of the event exactly once",
        )
        .into());
    }
    state
        .store()
        .set_image_positions(event.id, &order.image_ids)
        .await?;
    let images = state.store().event_images(event.id).await?;
    Ok(Json(
        images
            .into_iter()
            .map(|image| view(storage, image))
            .collect(),
    ))
}
