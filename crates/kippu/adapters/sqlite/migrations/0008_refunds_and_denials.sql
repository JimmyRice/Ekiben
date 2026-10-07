-- Refunds of issued tickets, and deny lists.

-- Until when buyers may refund tickets of a type themselves; NULL: they may not.
ALTER TABLE ticket_types ADD COLUMN refundable_until INTEGER;

-- Money returned for some of a reservation's tickets, which are revoked in the same
-- transaction. `pending` until the attestor confirms it returned the money.
CREATE TABLE refunds (
    id             BLOB    PRIMARY KEY,
    reservation_id BLOB    NOT NULL REFERENCES reservations (id),
    account_id     BLOB    NOT NULL REFERENCES accounts (id),
    event_id       BLOB    NOT NULL REFERENCES events (id),
    attestor_id    BLOB    NOT NULL,
    attestation_id TEXT    NOT NULL,
    ticket_ids     TEXT    NOT NULL, -- JSON array of ticket ids
    amount_minor   INTEGER NOT NULL CHECK (amount_minor >= 0),
    currency       TEXT    NOT NULL,
    reason         TEXT    NOT NULL CHECK (reason IN ('requested', 'reversal')),
    status         TEXT    NOT NULL CHECK (status IN ('pending', 'completed')),
    created_at     INTEGER NOT NULL,
    completed_at   INTEGER,
    FOREIGN KEY (attestor_id, attestation_id)
        REFERENCES payment_attestations (attestor_id, attestation_id)
) STRICT;

CREATE INDEX refunds_by_reservation ON refunds (reservation_id, created_at, id);

-- Deny lists: a ticket or an account refused entry to one event (event_id) or to every
-- event of the organization (event_id NULL). The id is derived from (organization, event,
-- subject), so adding the same denial twice names the same row.
CREATE TABLE denials (
    id              BLOB    PRIMARY KEY,
    organization_id BLOB    NOT NULL REFERENCES organizations (id),
    event_id        BLOB    REFERENCES events (id),
    subject_kind    TEXT    NOT NULL CHECK (subject_kind IN ('ticket', 'account')),
    subject_id      BLOB    NOT NULL,
    note            TEXT    NOT NULL,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    version         INTEGER NOT NULL
) STRICT;

CREATE INDEX denials_by_organization ON denials (organization_id, event_id, created_at, id);
CREATE INDEX denials_by_event ON denials (event_id, created_at, id);
CREATE INDEX denials_by_subject ON denials (subject_id, organization_id);

-- The tickets a gate must refuse are read per event, in id order.
CREATE INDEX tickets_by_event ON tickets (event_id, id);
