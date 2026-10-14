-- A buyer's tickets for one event, and their refunds.
CREATE INDEX tickets_by_account_event ON tickets (account_id, event_id, issued_at, id);
CREATE INDEX refunds_by_account ON refunds (account_id, created_at, id);
