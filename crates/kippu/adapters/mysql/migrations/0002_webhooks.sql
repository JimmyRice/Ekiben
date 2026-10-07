-- Webhooks: endpoints that receive outbox events, each with its own delivery cursor.

CREATE TABLE webhooks (
    id                BINARY(16)    PRIMARY KEY,
    organization_id   BINARY(16), -- NULL: global
    url               VARCHAR(2048) NOT NULL,
    topics            TEXT          NOT NULL, -- JSON array; empty: every topic
    active            BOOLEAN       NOT NULL,
    delivered_through BIGINT        NOT NULL,
    failures          INT UNSIGNED  NOT NULL,
    last_error        TEXT,
    next_attempt_at   BIGINT        NOT NULL,
    lease_until       BIGINT,
    created_at        BIGINT        NOT NULL,
    version           BIGINT        NOT NULL,
    INDEX webhooks_by_organization (organization_id),
    INDEX webhooks_due (active, next_attempt_at),
    FOREIGN KEY (organization_id) REFERENCES organizations (id) ON DELETE CASCADE
);
