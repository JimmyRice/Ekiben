-- Webhooks: endpoints that receive outbox events, each with its own delivery cursor.

CREATE TABLE webhooks (
    id                UUID        PRIMARY KEY,
    organization_id   UUID        REFERENCES organizations (id) ON DELETE CASCADE, -- NULL: global
    url               TEXT        NOT NULL,
    topics            JSONB       NOT NULL, -- array; empty: every topic
    active            BOOLEAN     NOT NULL,
    delivered_through BIGINT      NOT NULL,
    failures          BIGINT      NOT NULL CHECK (failures >= 0),
    last_error        TEXT,
    next_attempt_at   TIMESTAMPTZ NOT NULL,
    lease_until       TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL,
    version           BIGINT      NOT NULL
);

CREATE INDEX webhooks_by_organization ON webhooks (organization_id);
CREATE INDEX webhooks_due ON webhooks (next_attempt_at) WHERE active;
