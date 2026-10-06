//! HTTP handlers: reading uploads and streaming images are HTTP's business; what may be
//! uploaded and who sees what is the [`service`](super::service)'s.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, LOCATION};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use kippu_domain::image::ImageFormat;
use kippu_domain::{EventId, ImageId};

use super::dto::{EventImageView, ImageOrder};
use super::service::{self, IMMUTABLE, ImageContent, PublishedImage};
use crate::app::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult, Problem};
use crate::http::Json;

const TAG: &str = "images";

fn views(images: Vec<PublishedImage>) -> Json<Vec<EventImageView>> {
    Json(images.into_iter().map(EventImageView::from).collect())
}

/// Reads the whole body, which the router has already capped at `images.max_bytes`.
async fn read_body(body: Body) -> ApiResult<axum::body::Bytes> {
    axum::body::to_bytes(body, usize::MAX)
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
        })
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
    let declared = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<ImageFormat>().ok());
    let image =
        service::upload_image(&state, &principal, event_id, declared, read_body(body)).await?;
    Ok((StatusCode::CREATED, Json(image.into())))
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
    Ok(views(
        service::event_images(&state, principal.as_ref(), event_id).await?,
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
    match service::image_content(&state, principal.as_ref(), event_id, image_id).await? {
        ImageContent::Elsewhere(url) => {
            let location = HeaderValue::from_str(&url).map_err(ApiError::internal)?;
            Ok((StatusCode::FOUND, [(LOCATION, location)]).into_response())
        }
        ImageContent::Stored {
            format,
            public,
            object,
        } => {
            // Drafts are visible to their organizers only: keep them out of shared caches.
            let cache = if public {
                IMMUTABLE
            } else {
                "private, no-store"
            };
            Ok((
                [
                    (CONTENT_TYPE, HeaderValue::from_static(format.media_type())),
                    (CACHE_CONTROL, HeaderValue::from_static(cache)),
                ],
                Body::from_stream(object.body),
            )
                .into_response())
        }
    }
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
    service::delete_image(&state, &principal, event_id, image_id).await?;
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
    Ok(views(
        service::order_images(&state, &principal, event_id, &order.image_ids).await?,
    ))
}
