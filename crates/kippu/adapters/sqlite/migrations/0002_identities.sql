-- no-transaction
--
-- External sign-in (Sign in with Apple, WeChat, …): accounts may lack an email and a
-- password, and identities link a provider's subject to an account.
--
-- SQLite cannot relax NOT NULL in place, so `accounts` is rebuilt as its documentation
-- prescribes: with foreign keys off, inside one transaction, then checked.

PRAGMA foreign_keys = OFF;

BEGIN IMMEDIATE;

CREATE TABLE accounts_rebuilt (
    id            BLOB    PRIMARY KEY,
    email         TEXT    UNIQUE,
    display_name  TEXT    NOT NULL,
    role          TEXT    NOT NULL CHECK (role IN ('user', 'organizer', 'admin')),
    password_hash TEXT,
    created_at    INTEGER NOT NULL
) STRICT;

INSERT INTO accounts_rebuilt (id, email, display_name, role, password_hash, created_at)
SELECT id, email, display_name, role, password_hash, created_at FROM accounts;

DROP TABLE accounts;

ALTER TABLE accounts_rebuilt RENAME TO accounts;

CREATE TABLE identities (
    provider   TEXT    NOT NULL,
    subject    TEXT    NOT NULL,
    account_id BLOB    NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (provider, subject)
) STRICT;

CREATE INDEX identities_by_account ON identities (account_id, created_at);

COMMIT;

PRAGMA foreign_keys = ON;
