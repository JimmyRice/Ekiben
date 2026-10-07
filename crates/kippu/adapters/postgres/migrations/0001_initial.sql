-- Kippu schema for PostgreSQL.
--
-- Conventions: ids are native UUIDs; instants are TIMESTAMPTZ (microsecond precision, like
-- Kippu's Timestamp); money is BIGINT minor units; counts are BIGINT; small structured values
-- are JSONB. Every foreign key used to look rows up has an index.

-- ── Accounts ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE accounts (
    id            UUID        PRIMARY KEY,
    email         TEXT        UNIQUE,
    display_name  TEXT        NOT NULL,
    role          TEXT        NOT NULL CHECK (role IN ('user', 'organizer', 'admin')),
    password_hash TEXT,
    created_at    TIMESTAMPTZ NOT NULL
);

CREATE TABLE identities (
    provider   TEXT        NOT NULL,
    subject    TEXT        NOT NULL,
    account_id UUID        NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (provider, subject)
);

CREATE INDEX identities_by_account ON identities (account_id, created_at);

CREATE TABLE sessions (
    id                 UUID        PRIMARY KEY,
    account_id         UUID        NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    refresh_token_hash TEXT        NOT NULL UNIQUE,
    created_at         TIMESTAMPTZ NOT NULL,
    expires_at         TIMESTAMPTZ NOT NULL
);

CREATE INDEX sessions_by_account ON sessions (account_id);

CREATE TABLE organizations (
    id         UUID        PRIMARY KEY,
    slug       TEXT        NOT NULL UNIQUE,
    name       TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE memberships (
    organization_id UUID NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    account_id      UUID NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    PRIMARY KEY (organization_id, account_id)
);

CREATE INDEX memberships_by_account ON memberships (account_id);

-- ── Catalog ──────────────────────────────────────────────────────────────────────────────

CREATE TABLE events (
    id              UUID        PRIMARY KEY,
    organization_id UUID        NOT NULL REFERENCES organizations (id),
    slug            TEXT        NOT NULL UNIQUE,
    title           TEXT        NOT NULL,
    description     TEXT        NOT NULL,
    venue           TEXT        NOT NULL,
    starts_at       TIMESTAMPTZ NOT NULL,
    ends_at         TIMESTAMPTZ NOT NULL,
    status          TEXT        NOT NULL CHECK (status IN ('draft', 'published', 'cancelled')),
    created_at      TIMESTAMPTZ NOT NULL,
    updated_at      TIMESTAMPTZ NOT NULL,
    version         BIGINT      NOT NULL
);

CREATE INDEX events_by_organization ON events (organization_id);

CREATE TABLE sales (
    id                      UUID        PRIMARY KEY,
    event_id                UUID        NOT NULL REFERENCES events (id),
    name                    TEXT        NOT NULL,
    opens_at                TIMESTAMPTZ NOT NULL,
    closes_at               TIMESTAMPTZ NOT NULL,
    admission               JSONB       NOT NULL, -- AdmissionPolicy
    reservation_ttl_seconds BIGINT      NOT NULL,
    max_tickets_per_request BIGINT      NOT NULL,
    accepted_attestors      JSONB       NOT NULL, -- array of attestor ids
    environment             TEXT        NOT NULL CHECK (environment IN ('live', 'sandbox')),
    created_at              TIMESTAMPTZ NOT NULL,
    version                 BIGINT      NOT NULL
);

CREATE INDEX sales_by_event ON sales (event_id);

CREATE TABLE ticket_types (
    id                UUID        PRIMARY KEY,
    sale_id           UUID        NOT NULL REFERENCES sales (id),
    event_id          UUID        NOT NULL REFERENCES events (id),
    name              TEXT        NOT NULL,
    price_minor       BIGINT      NOT NULL CHECK (price_minor >= 0),
    currency          TEXT        NOT NULL,
    capacity          BIGINT      NOT NULL CHECK (capacity > 0),
    per_account_limit BIGINT      NOT NULL CHECK (per_account_limit > 0),
    valid_from        TIMESTAMPTZ NOT NULL,
    valid_until       TIMESTAMPTZ NOT NULL,
    ticket_extensions JSONB       NOT NULL, -- object: tag -> text
    created_at        TIMESTAMPTZ NOT NULL,
    version           BIGINT      NOT NULL
);

CREATE INDEX ticket_types_by_sale ON ticket_types (sale_id);

-- Kept apart from ticket_types so the hottest row in the system stays narrow.
CREATE TABLE inventory (
    ticket_type_id UUID   PRIMARY KEY REFERENCES ticket_types (id),
    capacity       BIGINT NOT NULL,
    held           BIGINT NOT NULL DEFAULT 0 CHECK (held >= 0),
    sold           BIGINT NOT NULL DEFAULT 0 CHECK (sold >= 0),
    CHECK (held + sold <= capacity)
);

CREATE TABLE quotas (
    account_id     UUID   NOT NULL,
    ticket_type_id UUID   NOT NULL REFERENCES ticket_types (id),
    taken          BIGINT NOT NULL CHECK (taken >= 0),
    PRIMARY KEY (account_id, ticket_type_id)
);

CREATE TABLE favorites (
    account_id UUID        NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    event_id   UUID        NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (account_id, event_id)
);

CREATE TABLE waiting_rooms (
    sale_id          UUID   PRIMARY KEY REFERENCES sales (id),
    last_position    BIGINT NOT NULL,
    admitted_through BIGINT NOT NULL,
    CHECK (admitted_through <= last_position)
);

-- ── Payments ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE attestors (
    id          UUID        PRIMARY KEY,
    name        TEXT        NOT NULL,
    environment TEXT        NOT NULL CHECK (environment IN ('live', 'sandbox')),
    revoked     BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL
);

-- The built-in attestors (see kippu_domain::AttestorId::FREE and ::MANUAL). They have no keys,
-- so nobody can sign as them over the API.
INSERT INTO attestors (id, name, environment, revoked, created_at) VALUES
    ('00000000-0000-0000-0000-000000000001', 'Free tickets', 'live', FALSE, 'epoch'),
    ('00000000-0000-0000-0000-000000000002', 'Manual (in person)', 'live', FALSE, 'epoch');

CREATE TABLE attestor_keys (
    attestor_id UUID        NOT NULL REFERENCES attestors (id),
    key_id      TEXT        NOT NULL,
    public_key  BYTEA       NOT NULL CHECK (octet_length(public_key) = 32),
    revoked     BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (attestor_id, key_id)
);

-- ── Purchasing ───────────────────────────────────────────────────────────────────────────

CREATE TABLE purchase_requests (
    id               UUID        PRIMARY KEY, -- derived from (account, sale, idempotency key)
    account_id       UUID        NOT NULL REFERENCES accounts (id),
    sale_id          UUID        NOT NULL REFERENCES sales (id),
    basket           JSONB       NOT NULL, -- array of line items
    status           TEXT        NOT NULL CHECK (status IN ('queued', 'reserved', 'rejected')),
    reservation_id   UUID,
    rejection_reason TEXT,
    lease_until      TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL,
    updated_at       TIMESTAMPTZ NOT NULL
);

CREATE INDEX purchase_requests_queue ON purchase_requests (created_at) WHERE status = 'queued';
CREATE INDEX purchase_requests_backlog ON purchase_requests (sale_id) WHERE status = 'queued';

CREATE TABLE reservations (
    id                  UUID        PRIMARY KEY,
    purchase_request_id UUID        NOT NULL UNIQUE REFERENCES purchase_requests (id),
    account_id          UUID        NOT NULL REFERENCES accounts (id),
    sale_id             UUID        NOT NULL REFERENCES sales (id),
    event_id            UUID        NOT NULL REFERENCES events (id),
    items               JSONB       NOT NULL, -- array of reserved items
    total_minor         BIGINT      NOT NULL CHECK (total_minor >= 0),
    currency            TEXT        NOT NULL,
    environment         TEXT        NOT NULL CHECK (environment IN ('live', 'sandbox')),
    status              TEXT        NOT NULL CHECK (status IN ('reserved', 'payment_pending',
                                                               'issued', 'expired', 'cancelled',
                                                               'refund_required', 'refunded')),
    attestor_id         UUID        REFERENCES attestors (id),
    expires_at          TIMESTAMPTZ NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL,
    updated_at          TIMESTAMPTZ NOT NULL
);

CREATE INDEX reservations_overdue ON reservations (expires_at)
    WHERE status IN ('reserved', 'payment_pending');
CREATE INDEX reservations_by_account ON reservations (account_id, created_at);

CREATE TABLE payment_attestations (
    attestor_id    UUID        NOT NULL REFERENCES attestors (id),
    attestation_id TEXT        NOT NULL,
    reservation_id UUID        NOT NULL REFERENCES reservations (id),
    amount_minor   BIGINT      NOT NULL CHECK (amount_minor >= 0),
    currency       TEXT        NOT NULL,
    occurred_at    TIMESTAMPTZ NOT NULL,
    received_at    TIMESTAMPTZ NOT NULL,
    disposition    TEXT        NOT NULL CHECK (disposition IN ('applied', 'refund_required',
                                                               'refunded')),
    PRIMARY KEY (attestor_id, attestation_id)
);

CREATE INDEX payment_attestations_by_reservation ON payment_attestations (reservation_id);

CREATE TABLE tickets (
    id             UUID        PRIMARY KEY,
    reservation_id UUID        NOT NULL REFERENCES reservations (id),
    account_id     UUID        NOT NULL REFERENCES accounts (id),
    event_id       UUID        NOT NULL REFERENCES events (id),
    ticket_type_id UUID        NOT NULL REFERENCES ticket_types (id),
    valid_from     TIMESTAMPTZ NOT NULL,
    valid_until    TIMESTAMPTZ NOT NULL,
    issued_at      TIMESTAMPTZ NOT NULL,
    status         TEXT        NOT NULL CHECK (status IN ('valid', 'revoked')),
    encoded        BYTEA       NOT NULL
);

CREATE INDEX tickets_by_account ON tickets (account_id, issued_at);
CREATE INDEX tickets_by_reservation ON tickets (reservation_id);

-- ── Housekeeping ─────────────────────────────────────────────────────────────────────────

-- Appends take a transaction-scoped advisory lock, so sequence numbers are assigned in
-- commit order and a reader that saw sequence n never later finds an event below n.
CREATE TABLE outbox (
    sequence   BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    topic      TEXT        NOT NULL,
    payload    JSONB       NOT NULL, -- IntegrationEvent
    created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE idempotency_records (
    scope       TEXT        NOT NULL,
    key         TEXT        NOT NULL,
    fingerprint TEXT        NOT NULL,
    status      INTEGER     NOT NULL,
    body        BYTEA       NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE audit_log (
    id     BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at     TIMESTAMPTZ NOT NULL,
    actor  TEXT        NOT NULL,
    action TEXT        NOT NULL,
    target TEXT        NOT NULL
);
