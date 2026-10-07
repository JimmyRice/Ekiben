-- Refunds of issued tickets, and deny lists.

-- Until when buyers may refund tickets of a type themselves; NULL: they may not.
ALTER TABLE ticket_types ADD COLUMN refundable_until BIGINT NULL;

-- Money returned for some of a reservation's tickets, which are revoked in the same
-- transaction. `pending` until the attestor confirms it returned the money.
CREATE TABLE refunds (
    id             BINARY(16)   PRIMARY KEY,
    reservation_id BINARY(16)   NOT NULL,
    account_id     BINARY(16)   NOT NULL,
    event_id       BINARY(16)   NOT NULL,
    attestor_id    BINARY(16)   NOT NULL,
    attestation_id VARCHAR(255) NOT NULL,
    ticket_ids     TEXT         NOT NULL, -- JSON array of ticket ids
    amount_minor   BIGINT       NOT NULL CHECK (amount_minor >= 0),
    currency       CHAR(3)      NOT NULL,
    reason         VARCHAR(16)  NOT NULL CHECK (reason IN ('requested', 'reversal')),
    status         VARCHAR(16)  NOT NULL CHECK (status IN ('pending', 'completed')),
    created_at     BIGINT       NOT NULL,
    completed_at   BIGINT,
    INDEX refunds_by_reservation (reservation_id, created_at, id),
    FOREIGN KEY (reservation_id) REFERENCES reservations (id),
    FOREIGN KEY (account_id) REFERENCES accounts (id),
    FOREIGN KEY (event_id) REFERENCES events (id),
    FOREIGN KEY (attestor_id, attestation_id)
        REFERENCES payment_attestations (attestor_id, attestation_id)
);

-- Deny lists: a ticket or an account refused entry to one event (event_id) or to every
-- event of the organization (event_id NULL). The id is derived from (organization, event,
-- subject), so adding the same denial twice names the same row. The subject is a ticket or
-- an account, so it has no foreign key.
CREATE TABLE denials (
    id              BINARY(16)  PRIMARY KEY,
    organization_id BINARY(16)  NOT NULL,
    event_id        BINARY(16),
    subject_kind    VARCHAR(16) NOT NULL CHECK (subject_kind IN ('ticket', 'account')),
    subject_id      BINARY(16)  NOT NULL,
    note            TEXT        NOT NULL,
    created_at      BIGINT      NOT NULL,
    updated_at      BIGINT      NOT NULL,
    version         BIGINT      NOT NULL,
    INDEX denials_by_organization (organization_id, event_id, created_at, id),
    INDEX denials_by_event (event_id, created_at, id),
    INDEX denials_by_subject (subject_id, organization_id),
    FOREIGN KEY (organization_id) REFERENCES organizations (id),
    FOREIGN KEY (event_id) REFERENCES events (id)
);

-- The tickets a gate must refuse are read per event, in id order.
CREATE INDEX tickets_by_event ON tickets (event_id, id);
