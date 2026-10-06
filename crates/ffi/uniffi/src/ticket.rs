use std::fmt::Write as _;

use kaisatsu::{Uuid, VerifiedTicket};

/// The claims of a verified ticket.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Ticket {
    /// Protocol version.
    pub version: u8,
    /// Id of the key that signed the ticket: 16 lowercase hex digits.
    pub key_id: String,
    /// Event the ticket admits to, as a lowercase hyphenated UUID.
    pub event_id: String,
    /// Unique ticket id, as a lowercase hyphenated UUID. Use it to detect re-use.
    pub ticket_id: String,
    /// Ticket type, as a lowercase hyphenated UUID.
    pub ticket_type_id: String,
    /// Start of validity, inclusive, in Unix seconds.
    pub valid_from: u64,
    /// End of validity, exclusive, in Unix seconds.
    pub valid_until: u64,
    /// Issue time in Unix seconds.
    pub issued_at: u64,
    /// The issuing Kippu deployment.
    pub issuer: String,
    /// Extension claims, in ticket order.
    pub extensions: Vec<Extension>,
}

/// One extension claim: a tag and its raw value.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Extension {
    /// The extension tag.
    pub tag: u8,
    /// The extension value.
    pub value: Vec<u8>,
}

impl From<&VerifiedTicket<'_>> for Ticket {
    fn from(ticket: &VerifiedTicket<'_>) -> Self {
        Self {
            version: ticket.version(),
            key_id: ticket.key_id().to_string(),
            event_id: uuid(ticket.event_id()),
            ticket_id: uuid(ticket.ticket_id()),
            ticket_type_id: uuid(ticket.ticket_type_id()),
            valid_from: ticket.valid_from(),
            valid_until: ticket.valid_until(),
            issued_at: ticket.issued_at(),
            issuer: ticket.issuer().to_owned(),
            extensions: ticket
                .extensions()
                .iter()
                .map(|extension| Extension {
                    tag: extension.tag,
                    value: extension.value.to_vec(),
                })
                .collect(),
        }
    }
}

fn uuid(id: Uuid) -> String {
    let mut text = String::with_capacity(36);
    for (index, byte) in id.to_bytes().into_iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        // Writing to a `String` cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text
}
