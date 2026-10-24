-- Organizations are edited with optimistic concurrency, like events; organizations and
-- accounts are listed for clients and admins.

ALTER TABLE organizations ADD COLUMN version BIGINT NOT NULL DEFAULT 1;

-- An organization's events, in either order of a listing; their leading column also serves
-- the check whether an organization has any public event.
CREATE INDEX events_by_organization_start ON events (organization_id, starts_at, id);
CREATE INDEX events_by_organization_creation ON events (organization_id, created_at, id);
DROP INDEX events_by_organization ON events;

-- Accounts filtered by role (email is already unique, so indexed).
CREATE INDEX accounts_by_role ON accounts (role, id);
