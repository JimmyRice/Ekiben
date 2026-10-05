-- Kippu schema for SQLite.
--
-- Conventions: ids are 16-byte UUID blobs; instants are INTEGER microseconds since the Unix
-- epoch (sortable and lossless); money is INTEGER minor units; small structured values are
-- JSON text. Tables are STRICT so SQLite enforces column types.

-- ── Accounts ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE accounts (
    id            BLOB    PRIMARY KEY,
    email         TEXT    NOT NULL UNIQUE,
    display_name  TEXT    NOT NULL,
    role          TEXT    NOT NULL CHECK (role IN ('user', 'organizer', 'admin')),
    password_hash TEXT    NOT NULL,
    created_at    INTEGER NOT NULL
) STRICT;

CREATE TABLE sessions (
    id                 BLOB    PRIMARY KEY,
    account_id         BLOB    NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    refresh_token_hash TEXT    NOT NULL UNIQUE,
    created_at         INTEGER NOT NULL,
    expires_at         INTEGER NOT NULL
) STRICT;

CREATE TABLE organizations (
    id         BLOB    PRIMARY KEY,
    slug       TEXT    NOT NULL UNIQUE,
    name       TEXT    NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE memberships (
    organization_id BLOB NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    account_id      BLOB NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    PRIMARY KEY (organization_id, account_id)
) STRICT;

CREATE INDEX memberships_by_account ON memberships (account_id);

-- ── Catalog ──────────────────────────────────────────────────────────────────────────────

CREATE TABLE events (
    id              BLOB    PRIMARY KEY,
    organization_id BLOB    NOT NULL REFERENCES organizations (id),
    slug            TEXT    NOT NULL UNIQUE,
    title           TEXT    NOT NULL,
    description     TEXT    NOT NULL,
    venue           TEXT    NOT NULL,
    starts_at       INTEGER NOT NULL,
    ends_at         INTEGER NOT NULL,
    status          TEXT    NOT NULL CHECK (status IN ('draft', 'published', 'cancelled')),
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    version         INTEGER NOT NULL
) STRICT;

CREATE INDEX events_by_organization ON events (organization_id);

CREATE TABLE sales (
    id                      BLOB    PRIMARY KEY,
    event_id                BLOB    NOT NULL REFERENCES events (id),
    name                    TEXT    NOT NULL,
    opens_at                INTEGER NOT NULL,
    closes_at               INTEGER NOT NULL,
    admission               TEXT    NOT NULL, -- JSON AdmissionPolicy
    reservation_ttl_seconds INTEGER NOT NULL,
    max_tickets_per_request INTEGER NOT NULL,
    accepted_attestors      TEXT    NOT NULL, -- JSON array of attestor ids
    environment             TEXT    NOT NULL CHECK (environment IN ('live', 'sandbox')),
    created_at              INTEGER NOT NULL,
    version                 INTEGER NOT NULL
) STRICT;

CREATE INDEX sales_by_event ON sales (event_id);

CREATE TABLE ticket_types (
    id                BLOB    PRIMARY KEY,
    sale_id           BLOB    NOT NULL REFERENCES sales (id),
    event_id          BLOB    NOT NULL REFERENCES events (id),
    name              TEXT    NOT NULL,
    price_minor       INTEGER NOT NULL CHECK (price_minor >= 0),
    currency          TEXT    NOT NULL,
    capacity          INTEGER NOT NULL CHECK (capacity > 0),
    per_account_limit INTEGER NOT NULL CHECK (per_account_limit > 0),
    valid_from        INTEGER NOT NULL,
    valid_until       INTEGER NOT NULL,
    ticket_extensions TEXT    NOT NULL, -- JSON object: tag -> text
    created_at        INTEGER NOT NULL,
    version           INTEGER NOT NULL
) STRICT;

CREATE INDEX ticket_types_by_sale ON ticket_types (sale_id);

-- Kept apart from ticket_types so the hottest row in the system stays narrow.
CREATE TABLE inventory (
    ticket_type_id BLOB    PRIMARY KEY REFERENCES ticket_types (id),
    capacity       INTEGER NOT NULL,
    held           INTEGER NOT NULL DEFAULT 0 CHECK (held >= 0),
    sold           INTEGER NOT NULL DEFAULT 0 CHECK (sold >= 0),
    CHECK (held + sold <= capacity)
) STRICT;

CREATE TABLE quotas (
    account_id     BLOB    NOT NULL,
    ticket_type_id BLOB    NOT NULL REFERENCES ticket_types (id),
    taken          INTEGER NOT NULL CHECK (taken >= 0),
    PRIMARY KEY (account_id, ticket_type_id)
) STRICT;

CREATE TABLE favorites (
    account_id BLOB    NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    event_id   BLOB    NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (account_id, event_id)
) STRICT;

CREATE TABLE waiting_rooms (
    sale_id          BLOB    PRIMARY KEY REFERENCES sales (id),
    last_position    INTEGER NOT NULL,
    admitted_through INTEGER NOT NULL,
    CHECK (admitted_through <= last_position)
) STRICT;

-- ── Payments ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE attestors (
    id          BLOB    PRIMARY KEY,
    name        TEXT    NOT NULL,
    environment TEXT    NOT NULL CHECK (environment IN ('live', 'sandbox')),
    revoked     INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1)),
    created_at  INTEGER NOT NULL
) STRICT;

-- The built-in attestors (see kippu_domain::AttestorId::FREE and ::MANUAL). They have no keys,
-- so nobody can sign as them over the API.
INSERT INTO attestors (id, name, environment, revoked, created_at) VALUES
    (X'00000000000000000000000000000001', 'Free tickets', 'live', 0, 0),
    (X'00000000000000000000000000000002', 'Manual (in person)', 'live', 0, 0);

CREATE TABLE attestor_keys (
    attestor_id BLOB    NOT NULL REFERENCES attestors (id),
    key_id      TEXT    NOT NULL,
    public_key  BLOB    NOT NULL CHECK (length(public_key) = 32),
    revoked     INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1)),
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (attestor_id, key_id)
) STRICT;

-- ── Purchasing ───────────────────────────────────────────────────────────────────────────

CREATE TABLE purchase_requests (
    id               BLOB    PRIMARY KEY, -- derived from (account, sale, idempotency key)
    account_id       BLOB    NOT NULL REFERENCES accounts (id),
    sale_id          BLOB    NOT NULL REFERENCES sales (id),
    basket           TEXT    NOT NULL, -- JSON array of line items
    status           TEXT    NOT NULL CHECK (status IN ('queued', 'reserved', 'rejected')),
    reservation_id   BLOB,
    rejection_reason TEXT,
    lease_until      INTEGER,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
) STRICT;

CREATE INDEX purchase_requests_queue ON purchase_requests (created_at) WHERE status = 'queued';
CREATE INDEX purchase_requests_backlog ON purchase_requests (sale_id) WHERE status = 'queued';

CREATE TABLE reservations (
    id                  BLOB    PRIMARY KEY,
    purchase_request_id BLOB    NOT NULL UNIQUE REFERENCES purchase_requests (id),
    account_id          BLOB    NOT NULL REFERENCES accounts (id),
    sale_id             BLOB    NOT NULL REFERENCES sales (id),
    event_id            BLOB    NOT NULL REFERENCES events (id),
    items               TEXT    NOT NULL, -- JSON array of reserved items
    total_minor         INTEGER NOT NULL CHECK (total_minor >= 0),
    currency            TEXT    NOT NULL,
    environment         TEXT    NOT NULL CHECK (environment IN ('live', 'sandbox')),
    status              TEXT    NOT NULL CHECK (status IN ('reserved', 'payment_pending', 'issued',
                                                           'expired', 'cancelled',
                                                           'refund_required', 'refunded')),
    attestor_id         BLOB    REFERENCES attestors (id),
    expires_at          INTEGER NOT NULL,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
) STRICT;

CREATE INDEX reservations_overdue ON reservations (expires_at)
    WHERE status IN ('reserved', 'payment_pending');
CREATE INDEX reservations_by_account ON reservations (account_id, created_at);

CREATE TABLE payment_attestations (
    attestor_id    BLOB    NOT NULL REFERENCES attestors (id),
    attestation_id TEXT    NOT NULL,
    reservation_id BLOB    NOT NULL REFERENCES reservations (id),
    amount_minor   INTEGER NOT NULL CHECK (amount_minor >= 0),
    currency       TEXT    NOT NULL,
    occurred_at    INTEGER NOT NULL,
    received_at    INTEGER NOT NULL,
    disposition    TEXT    NOT NULL CHECK (disposition IN ('applied', 'refund_required', 'refunded')),
    PRIMARY KEY (attestor_id, attestation_id)
) STRICT;

CREATE INDEX payment_attestations_by_reservation ON payment_attestations (reservation_id);

CREATE TABLE tickets (
    id             BLOB    PRIMARY KEY,
    reservation_id BLOB    NOT NULL REFERENCES reservations (id),
    account_id     BLOB    NOT NULL REFERENCES accounts (id),
    event_id       BLOB    NOT NULL REFERENCES events (id),
    ticket_type_id BLOB    NOT NULL REFERENCES ticket_types (id),
    valid_from     INTEGER NOT NULL,
    valid_until    INTEGER NOT NULL,
    issued_at      INTEGER NOT NULL,
    status         TEXT    NOT NULL CHECK (status IN ('valid', 'revoked')),
    encoded        BLOB    NOT NULL
) STRICT;

CREATE INDEX tickets_by_account ON tickets (account_id, issued_at);
CREATE INDEX tickets_by_reservation ON tickets (reservation_id);

-- ── Housekeeping ─────────────────────────────────────────────────────────────────────────

CREATE TABLE outbox (
    sequence   INTEGER PRIMARY KEY AUTOINCREMENT,
    topic      TEXT    NOT NULL,
    payload    TEXT    NOT NULL, -- JSON IntegrationEvent
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE idempotency_records (
    scope       TEXT    NOT NULL,
    key         TEXT    NOT NULL,
    fingerprint TEXT    NOT NULL,
    status      INTEGER NOT NULL,
    body        BLOB    NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
) STRICT;

CREATE TABLE audit_log (
    id     INTEGER PRIMARY KEY AUTOINCREMENT,
    at     INTEGER NOT NULL,
    actor  TEXT    NOT NULL,
    action TEXT    NOT NULL,
    target TEXT    NOT NULL
) STRICT;
