-- Who sees a sale: everyone (public), the accounts on its invitation list (invited), or only its
-- organizers, who grant its tickets (private). Sales so far are public.
ALTER TABLE sales ADD COLUMN audience TEXT NOT NULL DEFAULT 'public'
    CHECK (audience IN ('public', 'invited', 'private'));

-- The accounts a sale for invited accounts only is open to; listed newest first per sale.
CREATE TABLE sale_invitations (
    sale_id    UUID        NOT NULL REFERENCES sales (id),
    account_id UUID        NOT NULL REFERENCES accounts (id),
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (sale_id, account_id)
);

CREATE INDEX sale_invitations_by_time ON sale_invitations (sale_id, created_at, account_id);
