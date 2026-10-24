-- Who sees a sale: everyone (public), the accounts on its invitation list (invited), or only its
-- organizers, who grant its tickets (private). Sales so far are public.
ALTER TABLE sales ADD COLUMN audience VARCHAR(16) NOT NULL DEFAULT 'public'
    CHECK (audience IN ('public', 'invited', 'private'));

-- The accounts a sale for invited accounts only is open to; listed newest first per sale.
CREATE TABLE sale_invitations (
    sale_id    BINARY(16) NOT NULL,
    account_id BINARY(16) NOT NULL,
    created_at BIGINT     NOT NULL,
    PRIMARY KEY (sale_id, account_id),
    INDEX sale_invitations_by_time (sale_id, created_at, account_id),
    FOREIGN KEY (sale_id) REFERENCES sales (id),
    FOREIGN KEY (account_id) REFERENCES accounts (id)
);
