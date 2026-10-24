-- A waiting room remembers the position each account got, so joining again returns it instead
-- of handing out another one: one account holds one place in a sale's queue.

CREATE TABLE waiting_room_entries (
    sale_id    BLOB    NOT NULL REFERENCES sales (id),
    account_id BLOB    NOT NULL REFERENCES accounts (id),
    position   INTEGER NOT NULL,
    PRIMARY KEY (sale_id, account_id)
) STRICT;
