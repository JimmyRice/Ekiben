//! Delivering outbox events to webhooks.
//!
//! Each webhook has its own cursor into the outbox. A worker claims a due webhook (a lease,
//! so one worker at a time), POSTs the events after its cursor in order, and moves the cursor
//! past each event delivered — or passed over, when the topic or organization does not match.
//! The first failure ends the run: the cursor stays before that event, so order is kept and
//! nothing is lost, and the next attempt waits longer after every consecutive failure.

use std::collections::HashMap;
use std::time::{Duration as StdDuration, Instant};

use ed25519_dalek::SigningKey;
use kippu_domain::webhook::{MAX_ERROR_LEN, Webhook};
use kippu_domain::{Duration, OrganizationId, ReservationId};
use kippu_store::{BoxError, Lease, OutboxRecord, WebhookRun};
use serde_json::json;

use super::signature::{DELIVERY_HEADER, SIGNATURE_HEADER, sign};
use crate::app::AppState;
use crate::module::Progress;

/// Which organization an event belongs to, via its reservation's event; `None` if that can no
/// longer be told (the records are gone), in which case only global webhooks receive it.
async fn organization_of(
    state: &AppState,
    reservation: ReservationId,
    cache: &mut HashMap<ReservationId, Option<OrganizationId>>,
) -> Result<Option<OrganizationId>, BoxError> {
    if let Some(known) = cache.get(&reservation) {
        return Ok(*known);
    }
    let organization = match state.store().reservation(reservation).await? {
        Some(reservation) => state
            .store()
            .event(reservation.event_id)
            .await?
            .map(|event| event.organization_id),
        None => None,
    };
    cache.insert(reservation, organization);
    Ok(organization)
}

/// The body of one delivery.
fn body(webhook: &Webhook, record: &OutboxRecord) -> Result<Vec<u8>, BoxError> {
    Ok(serde_json::to_vec(&json!({
        "webhook_id": webhook.id,
        "sequence": record.sequence,
        "created_at": record.created_at,
        "event": record.event,
    }))?)
}

/// POSTs one event. Anything but a 2xx answer within the timeout is a failure.
async fn post(
    client: &reqwest::Client,
    key: &SigningKey,
    webhook: &Webhook,
    record: &OutboxRecord,
    now_unix: i64,
) -> Result<(), String> {
    let body = body(webhook, record).map_err(|error| error.to_string())?;
    let response = client
        .post(&webhook.url)
        .header("content-type", "application/json")
        .header(SIGNATURE_HEADER, sign(key, &webhook.url, now_unix, &body))
        .header(
            DELIVERY_HEADER,
            format!("{}:{}", webhook.id, record.sequence),
        )
        .body(body)
        .send()
        .await
        .map_err(|error| {
            // reqwest's message omits the cause ("error sending request"); keep it.
            let mut message = error.to_string();
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                message = format!("{message}: {cause}");
                source = cause.source();
            }
            message
        })?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("answered HTTP {}", response.status().as_u16()))
    }
}

fn truncate(mut error: String) -> String {
    if error.len() > MAX_ERROR_LEN {
        let mut end = MAX_ERROR_LEN;
        while !error.is_char_boundary(end) {
            end -= 1;
        }
        error.truncate(end);
    }
    error
}

/// Background task: claims one due webhook and delivers what it has not received yet.
/// Reports more work after every claim, so all due webhooks are served before resting.
pub(crate) async fn deliver(
    state: AppState,
    client: reqwest::Client,
) -> Result<Progress, BoxError> {
    let Some(key) = state.webhook_key().cloned() else {
        return Ok(Progress::Idle);
    };
    let config = &state.config().webhooks;
    let now = state.now();
    let lease = Lease {
        now,
        until: now + Duration::seconds(i64::from(config.lease_seconds)),
    };
    let Some(webhook) = state.store().claim_webhook(lease).await? else {
        return Ok(Progress::Idle);
    };
    // Stop starting deliveries while one more could still finish within the lease.
    let budget = StdDuration::from_secs(u64::from(config.lease_seconds)).saturating_sub(
        StdDuration::from_secs(config.timeout_seconds.saturating_add(1)),
    );
    let started = Instant::now();

    let records = state
        .store()
        .outbox_after(webhook.delivered_through, config.batch_size)
        .await?;
    let mut through = webhook.delivered_through;
    let mut delivered = 0_usize;
    let mut failure = None;
    let mut organizations = HashMap::new();
    for record in &records {
        if started.elapsed() > budget {
            break;
        }
        let relevant = webhook.wants(&record.event)
            && match webhook.organization_id {
                None => true,
                Some(organization) => {
                    organization_of(&state, record.event.reservation_id(), &mut organizations)
                        .await?
                        == Some(organization)
                }
            };
        if relevant {
            if let Err(error) =
                post(&client, &key, &webhook, record, state.now().unix_seconds()).await
            {
                failure = Some(error);
                break;
            }
            delivered += 1;
        }
        through = record.sequence;
    }

    let run = match failure {
        None => WebhookRun {
            delivered_through: through,
            failures: 0,
            last_error: None,
            next_attempt_at: state.now(),
        },
        Some(error) => {
            let failures = webhook.failures.saturating_add(1);
            tracing::warn!(
                webhook = %webhook.id,
                failures,
                %error,
                "webhook delivery failed; retrying later"
            );
            WebhookRun {
                delivered_through: through,
                failures,
                last_error: Some(truncate(error)),
                next_attempt_at: Webhook::retry_at(state.now(), failures),
            }
        }
    };
    if !state
        .store()
        .finish_webhook_run(webhook.id, lease.until, &run)
        .await?
    {
        tracing::warn!(webhook = %webhook.id, "webhook lease lapsed during delivery");
    }
    if delivered > 0 {
        tracing::info!(
            webhook = %webhook.id,
            delivered,
            through = run.delivered_through,
            "webhook events delivered"
        );
    }
    Ok(Progress::MoreWork)
}
