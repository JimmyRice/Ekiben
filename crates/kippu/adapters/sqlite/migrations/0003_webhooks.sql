-- Webhooks: endpoints that receive outbox events, each with its own delivery cursor.

CREATE TABLE webhooks (
    id                BLOB    PRIMARY KEY,
    organization_id   BLOB    REFERENCES organizations (id) ON DELETE CASCADE, -- NULL: global
    url               TEXT    NOT NULL,
    topics            TEXT    NOT NULL, -- JSON array; empty: every topic
    active            INTEGER NOT NULL CHECK (active IN (0, 1)),
    delivered_through INTEGER NOT NULL,
    failures          INTEGER NOT NULL CHECK (failures >= 0),
    last_error        TEXT,
    next_attempt_at   INTEGER NOT NULL,
    lease_until       INTEGER,
    created_at        INTEGER NOT NULL,
    version           INTEGER NOT NULL
) STRICT;

CREATE INDEX webhooks_by_organization ON webhooks (organization_id);
CREATE INDEX webhooks_due ON webhooks (next_attempt_at) WHERE active = 1;
