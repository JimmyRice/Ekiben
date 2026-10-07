//! Webhooks: endpoints that receive integration events as signed HTTP requests.

use serde::{Deserialize, Serialize};

use crate::outbox::IntegrationEvent;
use crate::validation::ValidationError;
use crate::{OrganizationId, Timestamp, WebhookId};

/// Longest accepted endpoint URL.
const MAX_URL_LEN: usize = 2_048;
/// Longest stored delivery error.
pub const MAX_ERROR_LEN: usize = 500;

/// An endpoint that receives integration events, in outbox order, at least once each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Webhook {
    /// Identity of the webhook.
    pub id: WebhookId,
    /// Whose events it receives: those of this organization's events, or every event when
    /// absent (registered by an admin).
    pub organization_id: Option<OrganizationId>,
    /// Where events are sent (`POST`). `https` unless the deployment allows plain HTTP.
    pub url: String,
    /// Topics it receives; empty means all of them.
    pub topics: Vec<String>,
    /// Whether events are delivered. Paused webhooks skip nothing: they resume where they
    /// stopped.
    pub active: bool,
    /// Delivery progress: the outbox sequence of the last event delivered or passed over.
    pub delivered_through: i64,
    /// Failed attempts since the last success.
    pub failures: u32,
    /// What went wrong last, if the last attempt failed.
    pub last_error: Option<String>,
    /// When delivery is next attempted, later after each failure.
    pub next_attempt_at: Timestamp,
    /// When it was registered.
    pub created_at: Timestamp,
    /// Optimistic concurrency version.
    pub version: i64,
}

impl Webhook {
    /// Checks the URL's shape and the topics. Whether the URL may be reached at all (scheme,
    /// private addresses) is the deployment's policy, checked where it is known.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let url = self.url.as_str();
        let scheme_ok = url.starts_with("https://") || url.starts_with("http://");
        if !scheme_ok || url.len() > MAX_URL_LEN || url.chars().any(char::is_whitespace) {
            return Err(ValidationError::new(
                "url",
                "must be an http(s) URL of at most 2048 characters",
            ));
        }
        if let Some(topic) = self
            .topics
            .iter()
            .find(|topic| !IntegrationEvent::TOPICS.contains(&topic.as_str()))
        {
            return Err(ValidationError::new(
                "topics",
                if topic.is_empty() {
                    "contains an empty topic"
                } else {
                    "contains an unknown topic"
                },
            ));
        }
        Ok(())
    }

    /// Whether the webhook receives `event` by topic. (The organization is checked separately:
    /// it takes a lookup.)
    pub fn wants(&self, event: &IntegrationEvent) -> bool {
        self.topics.is_empty() || self.topics.iter().any(|topic| topic == event.topic())
    }

    /// When to try again after `failures` consecutive failures: 2^n seconds, at most an hour.
    pub fn retry_at(now: Timestamp, failures: u32) -> Timestamp {
        let seconds = 2_i64.saturating_pow(failures.min(12)).min(3_600);
        now.saturating_add(crate::Duration::seconds(seconds))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReservationId;

    fn webhook(url: &str, topics: &[&str]) -> Webhook {
        Webhook {
            id: WebhookId::generate(),
            organization_id: None,
            url: url.to_owned(),
            topics: topics.iter().map(|&topic| topic.to_owned()).collect(),
            active: true,
            delivered_through: 0,
            failures: 0,
            last_error: None,
            next_attempt_at: Timestamp::UNIX_EPOCH,
            created_at: Timestamp::UNIX_EPOCH,
            version: 1,
        }
    }

    #[test]
    fn urls_and_topics_are_checked() {
        assert!(webhook("https://example.org/hook", &[]).validate().is_ok());
        assert!(webhook("ftp://example.org", &[]).validate().is_err());
        assert!(webhook("https://exa mple.org", &[]).validate().is_err());
        assert!(
            webhook("https://example.org", &["tickets.issued"])
                .validate()
                .is_ok()
        );
        assert!(
            webhook("https://example.org", &["tickets.sold"])
                .validate()
                .is_err()
        );
    }

    #[test]
    fn topics_filter_events() {
        let expired = IntegrationEvent::ReservationExpired {
            reservation_id: ReservationId::generate(),
        };
        assert!(webhook("https://a.example", &[]).wants(&expired));
        assert!(webhook("https://a.example", &["reservation.expired"]).wants(&expired));
        assert!(!webhook("https://a.example", &["tickets.issued"]).wants(&expired));
    }

    #[test]
    fn retries_back_off_up_to_an_hour() {
        let now = Timestamp::from_unix_seconds(1_000);
        assert_eq!(
            Webhook::retry_at(now, 1),
            Timestamp::from_unix_seconds(1_002)
        );
        assert_eq!(
            Webhook::retry_at(now, 5),
            Timestamp::from_unix_seconds(1_032)
        );
        assert_eq!(
            Webhook::retry_at(now, 40),
            Timestamp::from_unix_seconds(4_600)
        );
    }
}
