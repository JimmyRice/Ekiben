//! Strongly typed identifiers.
//!
//! Every entity has its own id type, so an `EventId` can never be passed where a `SaleId` is
//! expected. Ids are `UUIDv7`: unique without coordination and roughly time-ordered, which keeps
//! database indexes compact.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        #[cfg_attr(feature = "openapi", derive(utoipa::ToSchema), schema(value_type = String, format = Uuid))]
        pub struct $name(Uuid);

        impl $name {
            /// Generates a new, time-ordered id.
            pub fn generate() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wraps an existing UUID, e.g. one read from the database.
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The underlying UUID.
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(text).map(Self)
            }
        }

        impl From<Uuid> for $name {
            fn from(uuid: Uuid) -> Self {
                Self(uuid)
            }
        }
    };
}

define_id!(
    /// A user, organizer or admin account. Root has no account.
    AccountId
);
define_id!(
    /// An organization that runs events (主办方).
    OrganizationId
);
define_id!(
    /// A convention or other event.
    EventId
);
define_id!(
    /// A sale: a window during which ticket types of an event are sold.
    SaleId
);
define_id!(
    /// A kind of ticket within a sale, with its own price and capacity.
    TicketTypeId
);
define_id!(
    /// An asynchronous request to buy tickets. Derived from the idempotency key, never random.
    PurchaseRequestId
);
define_id!(
    /// A temporary hold on inventory, awaiting payment.
    ReservationId
);
define_id!(
    /// An issued ticket.
    TicketId
);
define_id!(
    /// A trusted party that attests payments.
    AttestorId
);
define_id!(
    /// A refresh-token session.
    SessionId
);
define_id!(
    /// An endpoint that receives integration events.
    WebhookId
);
define_id!(
    /// An image of an event, kept in object storage.
    ImageId
);
