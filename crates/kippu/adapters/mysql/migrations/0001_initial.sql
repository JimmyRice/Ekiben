-- Kippu schema for MySQL 8.0.16+ (InnoDB, utf8mb4).
--
-- Conventions: ids are BINARY(16) UUIDs; instants are BIGINT microseconds since the Unix epoch
-- (DATETIME and TIMESTAMP either lose the zone or stop in 2038); money is BIGINT minor units;
-- small structured values are JSON text. MySQL has no partial indexes, so queue indexes lead
-- with the status column instead.

-- ── Accounts ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE accounts (
    id            BINARY(16)   PRIMARY KEY,
    email         VARCHAR(254) UNIQUE,
    display_name  VARCHAR(400) NOT NULL,
    role          VARCHAR(16)  NOT NULL CHECK (role IN ('user', 'organizer', 'admin')),
    password_hash VARCHAR(255),
    created_at    BIGINT       NOT NULL
);

CREATE TABLE identities (
    provider   VARCHAR(32)  NOT NULL,
    subject    VARCHAR(255) NOT NULL,
    account_id BINARY(16)   NOT NULL,
    created_at BIGINT       NOT NULL,
    PRIMARY KEY (provider, subject),
    INDEX identities_by_account (account_id, created_at),
    FOREIGN KEY (account_id) REFERENCES accounts (id) ON DELETE CASCADE
);

CREATE TABLE sessions (
    id                 BINARY(16) PRIMARY KEY,
    account_id         BINARY(16) NOT NULL,
    refresh_token_hash CHAR(64)   NOT NULL UNIQUE,
    created_at         BIGINT     NOT NULL,
    expires_at         BIGINT     NOT NULL,
    FOREIGN KEY (account_id) REFERENCES accounts (id) ON DELETE CASCADE
);

CREATE TABLE organizations (
    id         BINARY(16)   PRIMARY KEY,
    slug       VARCHAR(64)  NOT NULL UNIQUE,
    name       VARCHAR(800) NOT NULL,
    created_at BIGINT       NOT NULL
);

CREATE TABLE memberships (
    organization_id BINARY(16) NOT NULL,
    account_id      BINARY(16) NOT NULL,
    PRIMARY KEY (organization_id, account_id),
    INDEX memberships_by_account (account_id),
    FOREIGN KEY (organization_id) REFERENCES organizations (id) ON DELETE CASCADE,
    FOREIGN KEY (account_id) REFERENCES accounts (id) ON DELETE CASCADE
);

-- ── Catalog ──────────────────────────────────────────────────────────────────────────────

CREATE TABLE events (
    id              BINARY(16)  PRIMARY KEY,
    organization_id BINARY(16)  NOT NULL,
    slug            VARCHAR(64) NOT NULL UNIQUE,
    title           TEXT        NOT NULL,
    description     MEDIUMTEXT  NOT NULL,
    venue           TEXT        NOT NULL,
    starts_at       BIGINT      NOT NULL,
    ends_at         BIGINT      NOT NULL,
    status          VARCHAR(16) NOT NULL CHECK (status IN ('draft', 'published', 'cancelled')),
    created_at      BIGINT      NOT NULL,
    updated_at      BIGINT      NOT NULL,
    version         BIGINT      NOT NULL,
    INDEX events_by_organization (organization_id),
    FOREIGN KEY (organization_id) REFERENCES organizations (id)
);

CREATE TABLE sales (
    id                      BINARY(16)   PRIMARY KEY,
    event_id                BINARY(16)   NOT NULL,
    name                    TEXT         NOT NULL,
    opens_at                BIGINT       NOT NULL,
    closes_at               BIGINT       NOT NULL,
    admission               TEXT         NOT NULL, -- JSON AdmissionPolicy
    reservation_ttl_seconds INT UNSIGNED NOT NULL,
    max_tickets_per_request INT UNSIGNED NOT NULL,
    accepted_attestors      TEXT         NOT NULL, -- JSON array of attestor ids
    environment             VARCHAR(16)  NOT NULL CHECK (environment IN ('live', 'sandbox')),
    created_at              BIGINT       NOT NULL,
    version                 BIGINT       NOT NULL,
    INDEX sales_by_event (event_id),
    FOREIGN KEY (event_id) REFERENCES events (id)
);

CREATE TABLE ticket_types (
    id                BINARY(16)   PRIMARY KEY,
    sale_id           BINARY(16)   NOT NULL,
    event_id          BINARY(16)   NOT NULL,
    name              TEXT         NOT NULL,
    price_minor       BIGINT       NOT NULL CHECK (price_minor >= 0),
    currency          CHAR(3)      NOT NULL,
    capacity          INT UNSIGNED NOT NULL CHECK (capacity > 0),
    per_account_limit INT UNSIGNED NOT NULL CHECK (per_account_limit > 0),
    valid_from        BIGINT       NOT NULL,
    valid_until       BIGINT       NOT NULL,
    ticket_extensions TEXT         NOT NULL, -- JSON object: tag -> text
    created_at        BIGINT       NOT NULL,
    version           BIGINT       NOT NULL,
    INDEX ticket_types_by_sale (sale_id),
    FOREIGN KEY (sale_id) REFERENCES sales (id),
    FOREIGN KEY (event_id) REFERENCES events (id)
);

-- Kept apart from ticket_types so the hottest row in the system stays narrow. Counts are
-- signed so that a subtraction below zero fails the CHECK instead of wrapping.
CREATE TABLE inventory (
    ticket_type_id BINARY(16) PRIMARY KEY,
    capacity       BIGINT     NOT NULL,
    held           BIGINT     NOT NULL DEFAULT 0 CHECK (held >= 0),
    sold           BIGINT     NOT NULL DEFAULT 0 CHECK (sold >= 0),
    CHECK (held + sold <= capacity),
    FOREIGN KEY (ticket_type_id) REFERENCES ticket_types (id)
);

CREATE TABLE quotas (
    account_id     BINARY(16) NOT NULL,
    ticket_type_id BINARY(16) NOT NULL,
    taken          BIGINT     NOT NULL CHECK (taken >= 0),
    PRIMARY KEY (account_id, ticket_type_id),
    FOREIGN KEY (ticket_type_id) REFERENCES ticket_types (id)
);

CREATE TABLE favorites (
    account_id BINARY(16) NOT NULL,
    event_id   BINARY(16) NOT NULL,
    created_at BIGINT     NOT NULL,
    PRIMARY KEY (account_id, event_id),
    FOREIGN KEY (account_id) REFERENCES accounts (id) ON DELETE CASCADE,
    FOREIGN KEY (event_id) REFERENCES events (id) ON DELETE CASCADE
);

CREATE TABLE waiting_rooms (
    sale_id          BINARY(16) PRIMARY KEY,
    last_position    BIGINT     NOT NULL,
    admitted_through BIGINT     NOT NULL,
    CHECK (admitted_through <= last_position),
    FOREIGN KEY (sale_id) REFERENCES sales (id)
);

-- ── Payments ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE attestors (
    id          BINARY(16)   PRIMARY KEY,
    name        VARCHAR(400) NOT NULL,
    environment VARCHAR(16)  NOT NULL CHECK (environment IN ('live', 'sandbox')),
    revoked     BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at  BIGINT       NOT NULL
);

-- The built-in attestors (see kippu_domain::AttestorId::FREE and ::MANUAL). They have no keys,
-- so nobody can sign as them over the API.
INSERT INTO attestors (id, name, environment, revoked, created_at) VALUES
    (UNHEX('00000000000000000000000000000001'), 'Free tickets', 'live', FALSE, 0),
    (UNHEX('00000000000000000000000000000002'), 'Manual (in person)', 'live', FALSE, 0);

CREATE TABLE attestor_keys (
    attestor_id BINARY(16)   NOT NULL,
    key_id      VARCHAR(128) NOT NULL,
    public_key  BINARY(32)   NOT NULL,
    revoked     BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at  BIGINT       NOT NULL,
    PRIMARY KEY (attestor_id, key_id),
    FOREIGN KEY (attestor_id) REFERENCES attestors (id)
);

-- ── Purchasing ───────────────────────────────────────────────────────────────────────────

CREATE TABLE purchase_requests (
    id               BINARY(16)  PRIMARY KEY, -- derived from (account, sale, idempotency key)
    account_id       BINARY(16)  NOT NULL,
    sale_id          BINARY(16)  NOT NULL,
    basket           TEXT        NOT NULL, -- JSON array of line items
    status           VARCHAR(16) NOT NULL CHECK (status IN ('queued', 'reserved', 'rejected')),
    reservation_id   BINARY(16),
    rejection_reason VARCHAR(64),
    lease_until      BIGINT,
    created_at       BIGINT      NOT NULL,
    updated_at       BIGINT      NOT NULL,
    INDEX purchase_requests_queue (status, created_at),
    INDEX purchase_requests_backlog (sale_id, status),
    FOREIGN KEY (account_id) REFERENCES accounts (id),
    FOREIGN KEY (sale_id) REFERENCES sales (id)
);

CREATE TABLE reservations (
    id                  BINARY(16)  PRIMARY KEY,
    purchase_request_id BINARY(16)  NOT NULL UNIQUE,
    account_id          BINARY(16)  NOT NULL,
    sale_id             BINARY(16)  NOT NULL,
    event_id            BINARY(16)  NOT NULL,
    items               TEXT        NOT NULL, -- JSON array of reserved items
    total_minor         BIGINT      NOT NULL CHECK (total_minor >= 0),
    currency            CHAR(3)     NOT NULL,
    environment         VARCHAR(16) NOT NULL CHECK (environment IN ('live', 'sandbox')),
    status              VARCHAR(16) NOT NULL CHECK (status IN ('reserved', 'payment_pending',
                                                               'issued', 'expired', 'cancelled',
                                                               'refund_required', 'refunded')),
    attestor_id         BINARY(16),
    expires_at          BIGINT      NOT NULL,
    created_at          BIGINT      NOT NULL,
    updated_at          BIGINT      NOT NULL,
    INDEX reservations_overdue (status, expires_at),
    INDEX reservations_by_account (account_id, created_at),
    FOREIGN KEY (purchase_request_id) REFERENCES purchase_requests (id),
    FOREIGN KEY (account_id) REFERENCES accounts (id),
    FOREIGN KEY (sale_id) REFERENCES sales (id),
    FOREIGN KEY (event_id) REFERENCES events (id),
    FOREIGN KEY (attestor_id) REFERENCES attestors (id)
);

CREATE TABLE payment_attestations (
    attestor_id    BINARY(16)   NOT NULL,
    attestation_id VARCHAR(255) NOT NULL,
    reservation_id BINARY(16)   NOT NULL,
    amount_minor   BIGINT       NOT NULL CHECK (amount_minor >= 0),
    currency       CHAR(3)      NOT NULL,
    occurred_at    BIGINT       NOT NULL,
    received_at    BIGINT       NOT NULL,
    disposition    VARCHAR(16)  NOT NULL CHECK (disposition IN ('applied', 'refund_required',
                                                                'refunded')),
    PRIMARY KEY (attestor_id, attestation_id),
    INDEX payment_attestations_by_reservation (reservation_id),
    FOREIGN KEY (attestor_id) REFERENCES attestors (id),
    FOREIGN KEY (reservation_id) REFERENCES reservations (id)
);

CREATE TABLE tickets (
    id             BINARY(16)  PRIMARY KEY,
    reservation_id BINARY(16)  NOT NULL,
    account_id     BINARY(16)  NOT NULL,
    event_id       BINARY(16)  NOT NULL,
    ticket_type_id BINARY(16)  NOT NULL,
    valid_from     BIGINT      NOT NULL,
    valid_until    BIGINT      NOT NULL,
    issued_at      BIGINT      NOT NULL,
    status         VARCHAR(16) NOT NULL CHECK (status IN ('valid', 'revoked')),
    encoded        BLOB        NOT NULL,
    INDEX tickets_by_account (account_id, issued_at),
    INDEX tickets_by_reservation (reservation_id),
    FOREIGN KEY (reservation_id) REFERENCES reservations (id),
    FOREIGN KEY (account_id) REFERENCES accounts (id),
    FOREIGN KEY (event_id) REFERENCES events (id),
    FOREIGN KEY (ticket_type_id) REFERENCES ticket_types (id)
);

-- ── Housekeeping ─────────────────────────────────────────────────────────────────────────

-- Appends lock the single row of outbox_lock until commit, so sequence numbers are assigned
-- in commit order and a reader that saw sequence n never later finds an event below n.
CREATE TABLE outbox_lock (
    id TINYINT PRIMARY KEY CHECK (id = 1)
);

INSERT INTO outbox_lock (id) VALUES (1);

CREATE TABLE outbox (
    sequence   BIGINT      AUTO_INCREMENT PRIMARY KEY,
    topic      VARCHAR(64) NOT NULL,
    payload    TEXT        NOT NULL, -- JSON IntegrationEvent
    created_at BIGINT      NOT NULL
);

CREATE TABLE idempotency_records (
    scope       VARCHAR(255)      NOT NULL,
    `key`       VARCHAR(255)      NOT NULL,
    fingerprint CHAR(64)          NOT NULL,
    status      SMALLINT UNSIGNED NOT NULL,
    body        MEDIUMBLOB        NOT NULL,
    created_at  BIGINT            NOT NULL,
    PRIMARY KEY (scope, `key`)
);

CREATE TABLE audit_log (
    id     BIGINT       AUTO_INCREMENT PRIMARY KEY,
    at     BIGINT       NOT NULL,
    actor  VARCHAR(255) NOT NULL,
    action VARCHAR(255) NOT NULL,
    target TEXT         NOT NULL
);
