-- A waiting room remembers the position each account got, so joining again returns it instead
-- of handing out another one: one account holds one place in a sale's queue.

CREATE TABLE waiting_room_entries (
    sale_id    BINARY(16) NOT NULL,
    account_id BINARY(16) NOT NULL,
    position   BIGINT     NOT NULL,
    PRIMARY KEY (sale_id, account_id),
    FOREIGN KEY (sale_id) REFERENCES sales (id),
    FOREIGN KEY (account_id) REFERENCES accounts (id)
);
