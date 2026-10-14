-- A waiting room remembers the position each account got, so joining again returns it instead
-- of handing out another one: one account holds one place in a sale's queue.

CREATE TABLE waiting_room_entries (
    sale_id    UUID   NOT NULL REFERENCES sales (id),
    account_id UUID   NOT NULL REFERENCES accounts (id),
    position   BIGINT NOT NULL,
    PRIMARY KEY (sale_id, account_id)
);
