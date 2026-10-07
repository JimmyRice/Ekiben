-- Keyset pagination reads each listing in index order, so a page costs the same however deep
-- it is. Tickets and reservations already have `(account_id, time)` indexes.

CREATE INDEX events_by_start ON events (starts_at, id);
CREATE INDEX events_by_creation ON events (created_at, id);
CREATE INDEX events_by_country ON events (address_country, starts_at);
CREATE INDEX favorites_by_account ON favorites (account_id, created_at);
