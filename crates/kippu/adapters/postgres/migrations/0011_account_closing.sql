-- Accounts show contact details their owners choose to share, and are closed rather than
-- deleted: a closed account keeps its id, so its orders, tickets and denials stay attached,
-- and the fingerprint (SHA-256) of its email, which no other account may take. Accounts
-- created before this migration get their fingerprint when they change email or close.

ALTER TABLE accounts ADD COLUMN contacts JSONB NOT NULL DEFAULT '{}'; -- object: name -> address
ALTER TABLE accounts ADD COLUMN email_hash TEXT;
ALTER TABLE accounts ADD COLUMN deleted_at TIMESTAMPTZ;
CREATE UNIQUE INDEX accounts_by_email_hash ON accounts (email_hash);
